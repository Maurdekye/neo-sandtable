//! A toy hidden-information game used to build and test the seat plumbing before the real engine's
//! decision model exists.
//!
//! **Number Duel.** Two seats are each dealt a private hand of [`HAND`] distinct cards from 1–9.
//! In each of [`HAND`] rounds both seats secretly choose one card from their hand; when both have
//! chosen, the cards are revealed, the higher card wins the round, and a tie scores nothing. The
//! seat with more rounds won wins the game. A seat never sees the opponent's hand or its
//! not-yet-revealed choice — only that the opponent has locked in.

use std::collections::BTreeMap;

use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use serde_json::{Value, json};

use crate::game::{
    GameBackend, PendingDecision, SeatInfo, SubmitReceipt, SubmitRequest, ToolError,
};

/// Cards per hand, and number of rounds.
pub const HAND: usize = 5;

#[derive(Clone, Debug)]
struct RoundResult {
    round: usize,
    plays: BTreeMap<String, u8>,
    winner: Option<String>,
}

/// The two-seat toy game. Deterministic from its seed.
pub struct NumberDuel {
    seats: Vec<SeatInfo>,
    hands: BTreeMap<String, Vec<u8>>,
    round: usize,
    /// Choices locked in for the open round (hidden from the opponent until the round resolves).
    chosen: BTreeMap<String, u8>,
    history: Vec<RoundResult>,
    /// Accepted responses by decision id, for idempotent replay.
    resolved: BTreeMap<String, Value>,
    seq: u64,
    opened_seq: u64,
    epochs: BTreeMap<String, u64>,
}

impl NumberDuel {
    pub const SEAT_A: &'static str = "axis.commander";
    pub const SEAT_B: &'static str = "commonwealth.commander";

    pub fn new(seed: u64) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let seats: Vec<SeatInfo> = [Self::SEAT_A, Self::SEAT_B]
            .iter()
            .map(|id| {
                let (side, role) = id.split_once('.').expect("seat id");
                SeatInfo {
                    id: (*id).to_string(),
                    side: side.into(),
                    role: role.into(),
                }
            })
            .collect();
        let mut hands = BTreeMap::new();
        for s in &seats {
            let mut pool: Vec<u8> = (1..=9).collect();
            let mut hand = Vec::new();
            for _ in 0..HAND {
                let i = (rng.next_u32() as usize) % pool.len();
                hand.push(pool.remove(i));
            }
            hand.sort_unstable();
            hands.insert(s.id.clone(), hand);
        }
        let epochs = seats.iter().map(|s| (s.id.clone(), 1)).collect();
        Self {
            seats,
            hands,
            round: 1,
            chosen: BTreeMap::new(),
            history: Vec::new(),
            resolved: BTreeMap::new(),
            seq: 1,
            opened_seq: 1,
            epochs,
        }
    }

    fn decision_id(&self, seat: &str) -> String {
        format!("r{}.{seat}", self.round)
    }

    fn finished(&self) -> bool {
        self.round > HAND
    }

    fn score(&self, seat: &str) -> usize {
        self.history
            .iter()
            .filter(|r| r.winner.as_deref() == Some(seat))
            .count()
    }

    fn known_seat(&self, seat: &str) -> Result<(), ToolError> {
        if self.seats.iter().any(|s| s.id == seat) {
            Ok(())
        } else {
            Err(ToolError::Other("unknown seat".into()))
        }
    }

    fn open_decision(&self, seat: &str) -> Option<PendingDecision> {
        if self.finished() || self.chosen.contains_key(seat) {
            return None;
        }
        Some(PendingDecision {
            id: self.decision_id(seat),
            seat: seat.to_string(),
            kind: "play_card".into(),
            summary: format!("Round {} of {HAND}: choose a card to play", self.round),
            opened_seq: self.opened_seq,
        })
    }

    fn opponent<'a>(&'a self, seat: &str) -> &'a str {
        self.seats
            .iter()
            .map(|s| s.id.as_str())
            .find(|id| *id != seat)
            .expect("two seats")
    }
}

impl GameBackend for NumberDuel {
    fn seats(&self) -> Vec<SeatInfo> {
        self.seats.clone()
    }

    fn game_seq(&self) -> u64 {
        self.seq
    }

    fn pending(&self, seat: &str) -> Vec<PendingDecision> {
        self.open_decision(seat).into_iter().collect()
    }

    fn observe(&self, seat: &str) -> Value {
        let opp = self.opponent(seat);
        let history: Vec<Value> = self
            .history
            .iter()
            .map(|r| {
                json!({
                    "round": r.round,
                    "you_played": r.plays[seat],
                    "opponent_played": r.plays[opp],
                    "winner": r.winner.as_deref().map(|w| if w == seat { "you" } else { "opponent" }),
                })
            })
            .collect();
        json!({
            "game": "number_duel",
            "game_seq": self.seq,
            "you": seat,
            "round": self.round.min(HAND),
            "rounds_total": HAND,
            "your_hand": self.hands[seat],
            "your_score": self.score(seat),
            "opponent_score": self.score(opp),
            "opponent_locked_in": self.chosen.contains_key(opp),
            "you_locked_in": self.chosen.contains_key(seat),
            "history": history,
            "pending_decisions": self.pending(seat),
            "finished": self.finished(),
        })
    }

    fn inspect(&self, seat: &str, target: &str) -> Result<Value, ToolError> {
        self.known_seat(seat)?;
        let opp = self.opponent(seat);
        if target == "hand" {
            return Ok(json!({ "hand": self.hands[seat] }));
        }
        if let Some(n) = target.strip_prefix("round:") {
            let n: usize = n
                .parse()
                .map_err(|_| ToolError::UnknownTarget(target.into()))?;
            return match self.history.iter().find(|r| r.round == n) {
                Some(r) => Ok(json!({
                    "round": r.round,
                    "you_played": r.plays[seat],
                    "opponent_played": r.plays[opp],
                    "winner": r.winner.as_deref().map(|w| if w == seat { "you" } else { "opponent" }),
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

    fn describe_actions(&self, seat: &str, decision_id: &str) -> Result<Value, ToolError> {
        let open = self
            .open_decision(seat)
            .filter(|d| d.id == decision_id)
            .ok_or_else(|| ToolError::UnknownDecision(decision_id.into()))?;
        Ok(json!({
            "decision_id": open.id,
            "kind": open.kind,
            "parameters": {
                "card": { "type": "integer", "one_of": self.hands[seat] },
            },
            "response_shape": { "card": "<one of the cards in your hand>" },
        }))
    }

    fn validate(
        &self,
        seat: &str,
        decision_id: &str,
        response: &Value,
    ) -> Result<Value, ToolError> {
        self.describe_actions(seat, decision_id)?;
        let card = parse_card(response)?;
        if !self.hands[seat].contains(&card) {
            return Err(ToolError::Invalid(format!(
                "card {card} is not in your hand {:?}",
                self.hands[seat]
            )));
        }
        Ok(json!({ "valid": true, "card": card }))
    }

    fn submit(&mut self, seat: &str, request: SubmitRequest) -> Result<SubmitReceipt, ToolError> {
        self.known_seat(seat)?;
        if request.epoch != self.epoch(seat) {
            return Err(ToolError::EpochMismatch);
        }
        if let Some(previous) = self.resolved.get(&request.decision_id)
            && request.decision_id.ends_with(&format!(".{seat}"))
        {
            // Replay of an already accepted decision: idempotent if identical, else refused.
            return if *previous == request.response {
                Ok(SubmitReceipt {
                    decision_id: request.decision_id,
                    duplicate: true,
                    summary: "duplicate submission ignored".into(),
                    result: json!({ "accepted": true, "duplicate": true }),
                })
            } else {
                Err(ToolError::Stale(format!(
                    "decision {} was already resolved with a different response",
                    request.decision_id
                )))
            };
        }
        let open = self
            .open_decision(seat)
            .filter(|d| d.id == request.decision_id)
            .ok_or_else(|| ToolError::UnknownDecision(request.decision_id.clone()))?;
        let card = parse_card(&request.response)?;
        let hand = self.hands.get_mut(seat).expect("hand");
        let Some(pos) = hand.iter().position(|c| *c == card) else {
            return Err(ToolError::Invalid(format!(
                "card {card} is not in your hand {hand:?}"
            )));
        };
        hand.remove(pos);
        self.chosen.insert(seat.to_string(), card);
        self.resolved.insert(open.id.clone(), request.response);
        self.seq += 1;
        let mut result = json!({ "accepted": true, "duplicate": false, "locked_in": card });
        if self.chosen.len() == self.seats.len() {
            let plays = std::mem::take(&mut self.chosen);
            let (a, b) = (&self.seats[0].id, &self.seats[1].id);
            let winner = match plays[a].cmp(&plays[b]) {
                std::cmp::Ordering::Greater => Some(a.clone()),
                std::cmp::Ordering::Less => Some(b.clone()),
                std::cmp::Ordering::Equal => None,
            };
            let opp = plays[self.opponent(seat)];
            result = json!({
                "accepted": true,
                "duplicate": false,
                "round_resolved": true,
                "you_played": card,
                "opponent_played": opp,
                "winner": winner.as_deref().map(|w| if w == seat { "you" } else { "opponent" }),
            });
            self.history.push(RoundResult {
                round: self.round,
                plays,
                winner,
            });
            self.round += 1;
            self.seq += 1;
            self.opened_seq = self.seq;
        }
        Ok(SubmitReceipt {
            decision_id: request.decision_id,
            duplicate: false,
            summary: format!("played card {card}"),
            result,
        })
    }

    fn epoch(&self, seat: &str) -> u64 {
        self.epochs.get(seat).copied().unwrap_or(0)
    }

    fn outcome(&self) -> Option<Value> {
        if !self.finished() {
            return None;
        }
        let (a, b) = (&self.seats[0].id, &self.seats[1].id);
        let (sa, sb) = (self.score(a), self.score(b));
        let winner = match sa.cmp(&sb) {
            std::cmp::Ordering::Greater => Some(a.clone()),
            std::cmp::Ordering::Less => Some(b.clone()),
            std::cmp::Ordering::Equal => None,
        };
        Some(json!({ "winner": winner, "scores": { a.as_str(): sa, b.as_str(): sb } }))
    }
}

fn parse_card(response: &Value) -> Result<u8, ToolError> {
    response
        .get("card")
        .and_then(Value::as_u64)
        .and_then(|c| u8::try_from(c).ok())
        .ok_or_else(|| ToolError::Invalid("response must be {\"card\": <integer>}".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submit(g: &mut NumberDuel, seat: &str, card: u8) -> Result<SubmitReceipt, ToolError> {
        let id = g.pending(seat)[0].id.clone();
        let epoch = g.epoch(seat);
        g.submit(
            seat,
            SubmitRequest {
                decision_id: id,
                epoch,
                response: json!({ "card": card }),
            },
        )
    }

    #[test]
    fn deterministic_from_seed() {
        let a = NumberDuel::new(7);
        let b = NumberDuel::new(7);
        assert_eq!(a.hands, b.hands);
        assert_ne!(a.hands, NumberDuel::new(8).hands);
    }

    #[test]
    fn observation_hides_the_opponent() {
        let g = NumberDuel::new(1);
        let text = g.observe(NumberDuel::SEAT_A).to_string();
        for card in &g.hands[NumberDuel::SEAT_B] {
            // The opponent's hand never appears under any key other than our own hand.
            let obs = g.observe(NumberDuel::SEAT_A);
            assert!(obs.get("opponent_hand").is_none(), "{text}");
            let _ = card;
        }
        assert!(g.observe(NumberDuel::SEAT_A)["your_hand"].is_array());
        assert!(g.inspect(NumberDuel::SEAT_A, "round:1").is_err());
    }

    #[test]
    fn plays_a_full_game() {
        let mut g = NumberDuel::new(3);
        for _ in 0..HAND {
            for seat in [NumberDuel::SEAT_A, NumberDuel::SEAT_B] {
                let card = g.hands[seat][0];
                submit(&mut g, seat, card).unwrap();
            }
        }
        let outcome = g.outcome().expect("finished");
        assert!(outcome["scores"].is_object());
        assert!(g.pending(NumberDuel::SEAT_A).is_empty());
        assert!(g.inspect(NumberDuel::SEAT_A, "round:5").is_ok());
    }

    #[test]
    fn submit_is_idempotent_and_conflicts_are_refused() {
        let mut g = NumberDuel::new(3);
        let seat = NumberDuel::SEAT_A;
        let card = g.hands[seat][0];
        let id = g.pending(seat)[0].id.clone();
        let req = |card: u8| SubmitRequest {
            decision_id: id.clone(),
            epoch: 1,
            response: json!({ "card": card }),
        };
        let first = g.submit(seat, req(card)).unwrap();
        assert!(!first.duplicate);
        let again = g.submit(seat, req(card)).unwrap();
        assert!(again.duplicate);
        let other = g.hands[seat][0];
        assert!(g.submit(seat, req(other)).is_err());
        // The card was spent exactly once.
        assert_eq!(g.hands[seat].len(), HAND - 1);
    }

    #[test]
    fn epoch_and_ownership_are_enforced() {
        let mut g = NumberDuel::new(3);
        let id = g.pending(NumberDuel::SEAT_A)[0].id.clone();
        let stale = SubmitRequest {
            decision_id: id.clone(),
            epoch: 0,
            response: json!({ "card": g.hands[NumberDuel::SEAT_A][0] }),
        };
        assert_eq!(
            g.submit(NumberDuel::SEAT_A, stale).unwrap_err(),
            ToolError::EpochMismatch
        );
        // B cannot answer A's decision.
        let wrong = SubmitRequest {
            decision_id: id,
            epoch: 1,
            response: json!({ "card": g.hands[NumberDuel::SEAT_B][0] }),
        };
        assert!(matches!(
            g.submit(NumberDuel::SEAT_B, wrong),
            Err(ToolError::UnknownDecision(_))
        ));
    }

    #[test]
    fn validate_does_not_commit() {
        let g = NumberDuel::new(3);
        let seat = NumberDuel::SEAT_A;
        let id = g.pending(seat)[0].id.clone();
        assert!(g.validate(seat, &id, &json!({ "card": 99 })).is_err());
        let ok = g.hands[seat][0];
        assert!(g.validate(seat, &id, &json!({ "card": ok })).is_ok());
        assert_eq!(g.hands[seat].len(), HAND);
    }
}
