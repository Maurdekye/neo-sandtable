//! A toy hidden-information game, implemented as a [`cna_core::engine::Ruleset`], used to build
//! and test the seat plumbing before CNA's decision model exists.
//!
//! **Number Duel.** Two seats are each dealt a private hand of [`HAND`] distinct cards from 1–9 by
//! the campaign RNG. In each of [`HAND`] rounds both seats secretly choose one card; once both
//! have chosen, the cards are revealed, the higher card wins the round and a tie scores nothing.
//! The seat with more rounds won wins. A seat never sees the opponent's hand or its not-yet-revealed
//! choice, only that the opponent has locked in.

use cna_core::clock::{Anchor, Clock};
use cna_core::decision::{
    ActionSchema, ActionSpace, ChoiceOption, DecisionRequest, DecisionResponse, Secrecy, Trigger,
};
use cna_core::engine::{Cx, EngineError, Progress, Rejection, Ruleset};
use cna_core::event::EngineEvent;
use cna_core::ids::{DecisionId, Role, SeatId, Side};
use cna_core::visibility::{Audience, Perspective};
use cna_protocol::{GameEvent, PendingDecision, ViewState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::game::{RulesetCore, RulesetGame, ToolError};

/// Cards per hand, and number of rounds.
pub const HAND: usize = 5;

pub const AXIS: SeatId = SeatId::new(Side::Axis, Role::Commander);
pub const COMMONWEALTH: SeatId = SeatId::new(Side::Commonwealth, Role::Commander);

fn index(seat: SeatId) -> Option<usize> {
    [AXIS, COMMONWEALTH].iter().position(|s| *s == seat)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundResult {
    pub round: usize,
    /// Cards played, indexed `[axis, commonwealth]`.
    pub plays: [u8; 2],
    /// 0 = axis, 1 = commonwealth, `None` = tie.
    pub winner: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuelState {
    pub dealt: bool,
    /// Hands, indexed `[axis, commonwealth]`.
    pub hands: [Vec<u8>; 2],
    /// 1-based; `HAND + 1` once finished.
    pub round: usize,
    /// Cards locked in for the open round.
    pub chosen: [Option<u8>; 2],
    pub history: Vec<RoundResult>,
}

impl DuelState {
    pub fn new() -> Self {
        Self {
            round: 1,
            ..Self::default()
        }
    }

    fn finished(&self) -> bool {
        self.round > HAND
    }

    fn score(&self, i: usize) -> usize {
        self.history.iter().filter(|r| r.winner == Some(i)).count()
    }

    fn decision_id(&self, i: usize) -> String {
        format!("r{}.{}", self.round, [AXIS, COMMONWEALTH][i])
    }
}

/// The ruleset. Stateless; the state is [`DuelState`].
pub struct NumberDuel;

impl NumberDuel {
    /// A ready toy game behind the seat-tool backend. Deterministic from `seed`.
    pub fn game(seed: u64) -> RulesetGame<NumberDuel> {
        RulesetGame::from_core(Self::core(seed))
    }

    /// The same game without the async wrapper (synchronous tests).
    pub fn core(seed: u64) -> RulesetCore<NumberDuel> {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&seed.to_le_bytes());
        RulesetCore::new(
            NumberDuel,
            (),
            DuelState::new(),
            bytes,
            vec![AXIS, COMMONWEALTH],
        )
        .expect("toy game starts")
        .with_inspector(Box::new(inspect))
    }

    fn clock(state: &DuelState) -> Clock {
        Clock::start(
            state.round.min(HAND) as u16,
            Anchor::new("sandbox.round.choose"),
        )
    }
}

impl Ruleset for NumberDuel {
    type State = DuelState;
    type Content = ();

    fn profile_id(&self) -> &str {
        "sandbox.number-duel.v1"
    }

    fn advance(
        &self,
        _content: &(),
        state: &mut DuelState,
        cx: &mut Cx<'_>,
    ) -> Result<Progress, EngineError> {
        if !state.dealt {
            for hand in &mut state.hands {
                let mut pool: Vec<u8> = (1..=9).collect();
                for _ in 0..HAND {
                    // Two dice give 0..36; the small bias is irrelevant for a toy.
                    let r = usize::from(cx.rng.d6().value() - 1) * 6
                        + usize::from(cx.rng.d6().value() - 1);
                    hand.push(pool.remove(r % pool.len()));
                }
                hand.sort_unstable();
            }
            state.dealt = true;
            cx.emit(EngineEvent::public(GameEvent::Note {
                text: format!("Each seat has been dealt {HAND} cards."),
            }));
        }
        if let [Some(a), Some(b)] = state.chosen {
            let winner = match a.cmp(&b) {
                std::cmp::Ordering::Greater => Some(0),
                std::cmp::Ordering::Less => Some(1),
                std::cmp::Ordering::Equal => None,
            };
            state.history.push(RoundResult {
                round: state.round,
                plays: [a, b],
                winner,
            });
            cx.emit(EngineEvent::public(GameEvent::Note {
                text: format!(
                    "Round {}: axis played {a}, commonwealth played {b}; {}.",
                    state.round,
                    match winner {
                        Some(0) => "axis wins the round",
                        Some(_) => "commonwealth wins the round",
                        None => "tie",
                    }
                ),
            }));
            state.chosen = [None, None];
            state.round += 1;
        }
        if state.finished() {
            let (a, b) = (state.score(0), state.score(1));
            let who = match a.cmp(&b) {
                std::cmp::Ordering::Greater => "axis wins",
                std::cmp::Ordering::Less => "commonwealth wins",
                std::cmp::Ordering::Equal => "drawn game",
            };
            return Ok(Progress::Finished {
                summary: format!("{who} {a}-{b}"),
            });
        }
        Ok(Progress::AwaitingDecisions)
    }

    fn respond(
        &self,
        _content: &(),
        state: &mut DuelState,
        response: &DecisionResponse,
        cx: &mut Cx<'_>,
    ) -> Result<(), Rejection> {
        let unknown = || Rejection::UnknownDecision {
            decision_id: response.decision_id.clone(),
        };
        if state.finished() {
            return Err(unknown());
        }
        let Some(me) = index(response.seat) else {
            return Err(unknown());
        };
        // Which seat does this decision id belong to?
        let owner = (0..2)
            .find(|i| state.decision_id(*i) == response.decision_id.as_str())
            .ok_or_else(unknown)?;
        if owner != me {
            return Err(Rejection::WrongSeat {
                decision_id: response.decision_id.clone(),
                seat: response.seat,
            });
        }
        if state.chosen[me].is_some() {
            return Err(unknown());
        }
        if response.decision_revision != 1 {
            return Err(Rejection::StaleRevision {
                expected: 1,
                got: response.decision_revision,
            });
        }
        // Models often send the id as a JSON number; accept both spellings of the same choice.
        let card: Option<u8> = match &response.action {
            Value::String(s) => s.trim().parse().ok(),
            Value::Number(n) => n.as_u64().and_then(|n| u8::try_from(n).ok()),
            _ => None,
        };
        let card = card.ok_or_else(|| Rejection::Illegal {
            message: "action must be the id of one card in your hand, e.g. \"5\"".into(),
        })?;
        let Some(pos) = state.hands[me].iter().position(|c| *c == card) else {
            return Err(Rejection::Illegal {
                message: format!("card {card} is not in your hand {:?}", state.hands[me]),
            });
        };
        state.hands[me].remove(pos);
        state.chosen[me] = Some(card);
        cx.emit(EngineEvent::new(
            Audience::Seat(response.seat),
            GameEvent::DecisionResolved {
                decision_id: response.decision_id.to_string(),
                seat: response.seat.to_string(),
                summary: format!("you locked in card {card}"),
                explanation: None,
            },
        ));
        cx.emit(EngineEvent::new(
            Audience::Operator,
            GameEvent::Note {
                text: format!("{} locked in {card}", response.seat),
            },
        ));
        Ok(())
    }

    fn pending(&self, _content: &(), state: &DuelState) -> Vec<DecisionRequest> {
        if state.finished() || !state.dealt {
            return Vec::new();
        }
        [AXIS, COMMONWEALTH]
            .into_iter()
            .enumerate()
            .filter(|(i, _)| state.chosen[*i].is_none())
            .map(|(i, seat)| DecisionRequest {
                id: DecisionId::new(state.decision_id(i)),
                seat,
                kind: "sandbox.play_card".into(),
                revision: 1,
                clock: Self::clock(state),
                summary: format!(
                    "Round {} of {HAND}: choose one card from your hand to play. Both seats choose \
                     secretly; the higher card wins the round.",
                    state.round
                ),
                rules: Vec::new(),
                trigger: Trigger::Scheduled,
                secrecy: Secrecy::SecretSimultaneous,
                space: ActionSpace::new(ActionSchema::Choice {
                    options: state.hands[i]
                        .iter()
                        .map(|c| ChoiceOption {
                            id: c.to_string(),
                            label: format!("card {c}"),
                            detail: None,
                        })
                        .collect(),
                }),
            })
            .collect()
    }

    fn observe(&self, _content: &(), state: &DuelState, perspective: Perspective) -> Value {
        let history = |me: Option<usize>| -> Vec<Value> {
            state
                .history
                .iter()
                .map(|r| match me {
                    Some(i) => json!({
                        "round": r.round,
                        "you_played": r.plays[i],
                        "opponent_played": r.plays[1 - i],
                        "winner": r.winner.map(|w| if w == i { "you" } else { "opponent" }),
                    }),
                    None => json!({
                        "round": r.round,
                        "axis_played": r.plays[0],
                        "commonwealth_played": r.plays[1],
                        "winner": r.winner.map(|w| ["axis", "commonwealth"][w]),
                    }),
                })
                .collect()
        };
        match perspective {
            Perspective::Seat(seat) => {
                let Some(me) = index(seat) else {
                    return json!({ "error": "not a seat of this game" });
                };
                json!({
                    "game": "number_duel",
                    "you": seat.to_string(),
                    "round": state.round.min(HAND),
                    "rounds_total": HAND,
                    "your_hand": state.hands[me],
                    "your_score": state.score(me),
                    "opponent_score": state.score(1 - me),
                    "you_locked_in": state.chosen[me].is_some(),
                    "opponent_locked_in": state.chosen[1 - me].is_some(),
                    "history": history(Some(me)),
                    "finished": state.finished(),
                })
            }
            Perspective::Side(side) => match [Side::Axis, Side::Commonwealth]
                .iter()
                .position(|s| *s == side)
            {
                Some(me) => json!({
                    "round": state.round.min(HAND),
                    "your_hand": state.hands[me],
                    "history": history(Some(me)),
                }),
                None => json!({}),
            },
            Perspective::Operator => json!({
                "operator_view": true,
                "round": state.round.min(HAND),
                "hands": { "axis": state.hands[0], "commonwealth": state.hands[1] },
                "locked_in": { "axis": state.chosen[0], "commonwealth": state.chosen[1] },
                "history": history(None),
            }),
        }
    }

    fn view(&self, _content: &(), state: &DuelState, perspective: Perspective) -> ViewState {
        let _ = perspective;
        ViewState {
            clock: cna_protocol::Clock {
                game_turn: state.round.min(HAND) as u16,
                date: "1940-09-15".into(),
                stage: "sandbox".into(),
                op_stage: None,
                phase: "choose".into(),
                segment: None,
                step: None,
                phasing: None,
            },
            stacks: Vec::new(),
            units: Default::default(),
            markers: Vec::new(),
            pending: self
                .pending(&(), state)
                .into_iter()
                .map(|d| PendingDecision {
                    id: d.id.to_string(),
                    seat: d.seat.to_string(),
                    kind: d.kind,
                    summary: d.summary,
                    opened_seq: 0,
                    rules: Vec::new(),
                    space: None,
                })
                .collect(),
        }
    }
}

fn inspect(_: &(), state: &DuelState, seat: SeatId, target: &str) -> Result<Value, ToolError> {
    let Some(me) = index(seat) else {
        return Err(ToolError::Other("not a seat of this game".into()));
    };
    if target == "hand" {
        return Ok(json!({ "hand": state.hands[me] }));
    }
    if let Some(n) = target.strip_prefix("round:") {
        let n: usize = n
            .parse()
            .map_err(|_| ToolError::UnknownTarget(target.into()))?;
        return match state.history.iter().find(|r| r.round == n) {
            Some(r) => Ok(json!({
                "round": r.round,
                "you_played": r.plays[me],
                "opponent_played": r.plays[1 - me],
                "winner": r.winner.map(|w| if w == me { "you" } else { "opponent" }),
            })),
            None => Err(ToolError::UnknownTarget(format!(
                "{target} (only resolved rounds can be inspected)"
            ))),
        };
    }
    Err(ToolError::UnknownTarget(format!(
        "{target} (try `hand` or `round:<n>`)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::SubmitRequest;

    fn submit(
        g: &mut RulesetCore<NumberDuel>,
        seat: SeatId,
        action: Value,
        key: &str,
    ) -> Result<crate::game::SubmitReceipt, ToolError> {
        let d = g.pending(seat).first().cloned();
        let id = d.map(|d| d.id.to_string()).unwrap_or_else(|| "none".into());
        let epoch = g.epoch(seat);
        g.submit(
            seat,
            SubmitRequest {
                decision_id: id,
                epoch,
                revision: None,
                idempotency_key: key.into(),
                action,
                public_explanation: None,
            },
        )
    }

    fn first_card(g: &RulesetCore<NumberDuel>, seat: SeatId) -> String {
        g.observe(seat)["your_hand"][0].to_string()
    }

    #[test]
    fn deterministic_from_seed() {
        let a = NumberDuel::core(7);
        let b = NumberDuel::core(7);
        assert_eq!(a.observe(AXIS), b.observe(AXIS));
        assert_ne!(
            a.observe(AXIS)["your_hand"],
            NumberDuel::core(8).observe(AXIS)["your_hand"]
        );
        assert_eq!(a.observe(AXIS)["your_hand"].as_array().unwrap().len(), HAND);
    }

    #[test]
    fn observation_hides_the_opponent() {
        let g = NumberDuel::core(1);
        let a = g.observe(AXIS);
        let b = g.observe(COMMONWEALTH);
        assert_ne!(a["your_hand"], b["your_hand"]);
        assert!(a.get("hands").is_none());
        assert!(g.inspect(AXIS, "round:1").is_err());
    }

    #[test]
    fn plays_a_full_game() {
        let mut g = NumberDuel::core(3);
        for round in 0..HAND {
            for seat in [AXIS, COMMONWEALTH] {
                let card = first_card(&g, seat);
                let action = Value::String(card.clone());
                submit(&mut g, seat, action, &format!("{seat}-{round}")).unwrap();
            }
        }
        assert!(g.outcome().is_some());
        assert!(g.pending(AXIS).is_empty());
        assert!(g.inspect(AXIS, "round:5").is_ok());
    }

    #[test]
    fn submit_is_idempotent_and_spends_the_card_once() {
        let mut g = NumberDuel::core(3);
        let card = first_card(&g, AXIS);
        let first = submit(&mut g, AXIS, json!(card), "k1").unwrap();
        assert!(!first.duplicate);
        let again = submit(&mut g, AXIS, json!(card), "k1").unwrap();
        assert!(again.duplicate);
        assert_eq!(
            g.observe(AXIS)["your_hand"].as_array().unwrap().len(),
            HAND - 1
        );
        // A different key for the resolved decision finds nothing pending.
        assert!(submit(&mut g, AXIS, json!(card), "k2").is_err());
    }

    #[test]
    fn epoch_ownership_and_legality_are_enforced() {
        let mut g = NumberDuel::core(3);
        let id = g.pending(AXIS)[0].id.to_string();
        let req = |epoch: u64, action: Value| SubmitRequest {
            decision_id: id.clone(),
            epoch,
            revision: None,
            idempotency_key: format!("e{epoch}"),
            action,
            public_explanation: None,
        };
        assert_eq!(
            g.submit(AXIS, req(0, json!("1"))).unwrap_err(),
            ToolError::EpochMismatch
        );
        // The other seat cannot answer it, and is told nothing beyond "unknown".
        assert!(matches!(
            g.submit(COMMONWEALTH, req(1, json!("1"))),
            Err(ToolError::UnknownDecision(_))
        ));
        assert!(matches!(
            g.submit(AXIS, req(1, json!("99"))),
            Err(ToolError::Illegal(_))
        ));
        assert!(matches!(
            g.submit(AXIS, req(1, json!([5]))),
            Err(ToolError::Illegal(_))
        ));
        assert_eq!(
            g.pending(AXIS).len(),
            1,
            "failed submissions changed nothing"
        );
    }

    #[test]
    fn validate_does_not_commit() {
        let g = NumberDuel::core(3);
        let id = g.pending(AXIS)[0].id.to_string();
        assert!(g.validate(AXIS, &id, &json!("99")).is_err());
        let ok = first_card(&g, AXIS);
        assert!(g.validate(AXIS, &id, &json!(ok)).is_ok());
        assert_eq!(g.observe(AXIS)["your_hand"].as_array().unwrap().len(), HAND);
    }

    #[test]
    fn round_result_reaches_the_seat_in_the_submit_result() {
        let mut g = NumberDuel::core(3);
        let (a, b) = (first_card(&g, AXIS), first_card(&g, COMMONWEALTH));
        submit(&mut g, AXIS, json!(a), "a").unwrap();
        let second = submit(&mut g, COMMONWEALTH, json!(b), "b").unwrap();
        let events = second.result["events"].to_string();
        assert!(events.contains("Round 1"), "{events}");
        // The operator-only note about who locked in what is not in a seat's events.
        assert!(!events.contains("axis.commander locked in"), "{events}");
    }
}
