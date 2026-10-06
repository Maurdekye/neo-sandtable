use super::*;
use cna_content::map::MapContent;
use cna_core::dice::CampaignRng;
use cna_core::engine::{Command, Game, evaluate};

fn content() -> SandboxContent {
    let map = MapContent::load(&cna_content::repo_data_dir().join("map")).expect("map loads");
    SandboxContent::from_map(&map).expect("sandbox region")
}

fn pick(rng: &mut CampaignRng, n: usize) -> usize {
    let x = (0..4).fold(0usize, |acc, _| acc * 6 + usize::from(rng.d6().value() - 1));
    x % n.max(1)
}

/// A legal-random controller driven only by the action schema.
fn random_value(schema: &ActionSchema, rng: &mut CampaignRng) -> Value {
    match schema {
        ActionSchema::Choice { options } => json!(options[pick(rng, options.len())].id),
        ActionSchema::Integer { min, max } => {
            json!(min + pick(rng, (max - min + 1) as usize) as i64)
        }
        ActionSchema::Bool => json!(rng.d6().value() > 3),
        ActionSchema::Unit { among } => json!(among[pick(rng, among.len())].as_str()),
        ActionSchema::Hex { among: Some(h) } => json!(h[pick(rng, h.len())].as_str()),
        ActionSchema::Hex { among: None } | ActionSchema::Path { .. } => Value::Null,
        ActionSchema::Record { fields } => {
            let mut o = serde_json::Map::new();
            for f in fields {
                if !f.optional || rng.d6().value() > 2 {
                    o.insert(f.name.clone(), random_value(&f.schema, rng));
                }
            }
            Value::Object(o)
        }
        ActionSchema::List { item, min, max } => {
            let n = *min as usize + pick(rng, (*max - *min + 1) as usize);
            Value::Array((0..n).map(|_| random_value(item, rng)).collect())
        }
    }
}

struct Played {
    events: Vec<EngineEvent>,
    rejections: usize,
    decisions: usize,
    summary: String,
    final_state: State,
}

fn play(content: &SandboxContent, game_seed: u8, player_seed: u8) -> Played {
    play_with(content, game_seed, player_seed, &[])
}

fn play_with(
    content: &SandboxContent,
    game_seed: u8,
    player_seed: u8,
    aggressive: &[Side],
) -> Played {
    let ruleset = Sandbox;
    let mut game: Game<Sandbox> = Game {
        state: State::new(content),
        rng: CampaignRng::from_seed([game_seed; 32]).state(),
    };
    let mut players = CampaignRng::from_seed([player_seed; 32]);
    let mut events = Vec::new();
    let (mut rejections, mut decisions) = (0, 0);
    for _ in 0..20_000 {
        let t = evaluate(&ruleset, content, &game, &Command::Advance).expect("advance");
        game = t.game;
        events.extend(t.events);
        if let Some(Progress::Finished { summary }) = t.progress {
            return Played {
                events,
                rejections,
                decisions,
                summary,
                final_state: game.state,
            };
        }
        let pending = ruleset.pending(content, &game.state);
        let req = pending
            .first()
            .expect("awaiting means something is pending")
            .clone();
        let mut answered = false;
        for attempt in 0..20 {
            let pass_now = req.space.pass.is_some() && (attempt >= 10 || players.d6().value() == 1);
            let action = if attempt == 0 && aggressive.contains(&req.seat.side) {
                crate::baseline::aggressive(content, &game.state, &req)
            } else if pass_now {
                Value::Null
            } else {
                random_value(&req.space.schema, &mut players)
            };
            let response = DecisionResponse {
                decision_id: req.id.clone(),
                seat: req.seat,
                controller_epoch: 1,
                decision_revision: req.revision,
                idempotency_key: format!("{}-{attempt}", req.id),
                action,
                public_explanation: None,
            };
            match evaluate(&ruleset, content, &game, &Command::Respond(response)) {
                Ok(t) => {
                    game = t.game;
                    events.extend(t.events);
                    decisions += 1;
                    answered = true;
                    break;
                }
                Err(Rejection::Illegal { .. }) => rejections += 1,
                Err(other) => panic!("unexpected rejection: {other}"),
            }
        }
        assert!(answered, "no legal answer found for {req:?}");
    }
    panic!("game did not finish");
}

#[test]
fn a_random_game_runs_to_completion() {
    let content = content();
    let played = play(&content, 1, 2);
    assert!(played.decisions > 20, "decisions: {}", played.decisions);
    assert!(!played.summary.is_empty());
    assert!(played.final_state.result.is_some());
    // Every kind of decision should have come up at least once in a full game.
    let kinds: BTreeSet<String> = played
        .events
        .iter()
        .filter_map(|e| match &e.event {
            GameEvent::DecisionOpened { decision } => Some(decision.kind.clone()),
            _ => None,
        })
        .collect();
    for k in ["sandbox.initiative", "sandbox.supply", "sandbox.movement"] {
        assert!(kinds.contains(k), "missing {k}: {kinds:?}");
    }
    let _ = played.rejections;
}

#[test]
fn games_are_deterministic() {
    let content = content();
    let a = play(&content, 9, 4);
    let b = play(&content, 9, 4);
    let ja = serde_json::to_string(&a.events).unwrap();
    let jb = serde_json::to_string(&b.events).unwrap();
    assert_eq!(ja, jb);
}

#[test]
fn many_seeds_cover_combat_and_reactions() {
    let content = content();
    let mut kinds = BTreeSet::new();
    let mut combats = 0;
    for seed in 0..12u8 {
        let aggressive: &[Side] = match seed % 3 {
            0 => &[Side::Axis, Side::Commonwealth],
            1 => &[Side::Axis],
            _ => &[Side::Commonwealth],
        };
        let played = play_with(&content, seed, seed.wrapping_add(100), aggressive);
        for e in &played.events {
            match &e.event {
                GameEvent::DecisionOpened { decision } => {
                    kinds.insert(decision.kind.clone());
                }
                GameEvent::CombatResolved { .. } => combats += 1,
                _ => {}
            }
        }
    }
    for k in [
        "sandbox.reaction",
        "sandbox.air",
        "sandbox.assault",
        "sandbox.repair",
    ] {
        assert!(kinds.contains(k), "never saw {k}: {kinds:?}");
    }
    assert!(combats > 0);
}

#[test]
fn hidden_information_does_not_leak() {
    let content = content();
    let played = play(&content, 3, 5);
    for e in &played.events {
        if let Audience::SideOnly(_) = e.audience {
            match &e.event {
                GameEvent::StackUpdated { stack } => {
                    assert!(stack.unit_ids.is_empty() && stack.visible_count.is_none())
                }
                GameEvent::StackRemoved { .. } => {}
                other => panic!("unexpected side-only event {other:?}"),
            }
        }
    }
    for side in Side::ALL {
        let p = Perspective::Side(side);
        let enemy = side.opponent();
        for e in played.events.iter().filter(|e| e.visible_to(p)) {
            match &e.event {
                GameEvent::UnitMoved { unit_id, .. } | GameEvent::UnitRemoved { unit_id, .. } => {
                    let own = played_unit_side(&content, unit_id);
                    assert_eq!(own, side, "{side} saw enemy unit event {:?}", e.event);
                }
                GameEvent::UnitUpdated { unit } => assert_eq!(unit.side, side),
                GameEvent::StackUpdated { stack } if stack.side == enemy => {
                    assert!(stack.unit_ids.is_empty(), "{side} saw enemy stack contents");
                }
                GameEvent::DecisionOpened { decision } => {
                    assert!(decision.seat.starts_with(side.as_str()));
                }
                _ => {}
            }
        }
        let view = Sandbox.view(&content, &played.final_state, p);
        assert!(view.units.values().all(|u| u.side == side));
        assert!(
            view.stacks
                .iter()
                .filter(|s| s.side == enemy)
                .all(|s| s.unit_ids.is_empty() && s.visible_count.is_none())
        );
    }
    // The operator never receives a redacted duplicate.
    for e in played
        .events
        .iter()
        .filter(|e| e.visible_to(Perspective::Operator))
    {
        if let GameEvent::StackUpdated { stack } = &e.event {
            assert!(!stack.unit_ids.is_empty());
        }
    }
}

fn played_unit_side(content: &SandboxContent, unit_id: &str) -> Side {
    content
        .setup
        .iter()
        .find(|u| u.id.as_str() == unit_id)
        .map(|u| u.side)
        .expect("known unit")
}

#[test]
fn bad_answers_are_rejected_without_change() {
    let content = content();
    let ruleset = Sandbox;
    let game: Game<Sandbox> = Game {
        state: State::new(&content),
        rng: CampaignRng::from_seed([1; 32]).state(),
    };
    let t = evaluate(&ruleset, &content, &game, &Command::Advance).unwrap();
    let game = t.game;
    let req = ruleset.pending(&content, &game.state).remove(0);
    assert_eq!(req.kind, "sandbox.initiative");

    let base = DecisionResponse {
        decision_id: req.id.clone(),
        seat: req.seat,
        controller_epoch: 1,
        decision_revision: 1,
        idempotency_key: "k".into(),
        action: json!("first"),
        public_explanation: None,
    };
    let wrong_seat = DecisionResponse {
        seat: SeatId::new(Side::Commonwealth, Role::Commander),
        ..base.clone()
    };
    assert!(matches!(
        evaluate(&ruleset, &content, &game, &Command::Respond(wrong_seat)),
        Err(Rejection::WrongSeat { .. })
    ));
    let bad_choice = DecisionResponse {
        action: json!("sideways"),
        ..base.clone()
    };
    assert!(matches!(
        evaluate(&ruleset, &content, &game, &Command::Respond(bad_choice)),
        Err(Rejection::Illegal { .. })
    ));
    let no_pass = DecisionResponse {
        action: Value::Null,
        ..base.clone()
    };
    assert!(matches!(
        evaluate(&ruleset, &content, &game, &Command::Respond(no_pass)),
        Err(Rejection::Illegal { .. })
    ));
    assert!(evaluate(&ruleset, &content, &game, &Command::Respond(base)).is_ok());
}

#[test]
fn action_spaces_render_as_json_schema() {
    let content = content();
    let ruleset = Sandbox;
    let game: Game<Sandbox> = Game {
        state: State::new(&content),
        rng: CampaignRng::from_seed([2; 32]).state(),
    };
    let t = evaluate(&ruleset, &content, &game, &Command::Advance).unwrap();
    for req in ruleset.pending(&content, &t.game.state) {
        let schema = req.space.to_json_schema();
        assert!(schema.is_object());
    }
    let obs = ruleset.observe(
        &content,
        &t.game.state,
        Perspective::Seat(SeatId::new(Side::Axis, Role::Commander)),
    );
    assert_eq!(obs["ruleset"], PROFILE_ID);
    assert!(
        obs["visible_units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["side"] == "axis")
    );
}

#[test]
fn turn_dates_advance_weekly() {
    assert_eq!(turn_date(1), "1940-09-15");
    assert_eq!(turn_date(2), "1940-09-22");
    assert_eq!(turn_date(3), "1940-09-29");
    assert_eq!(turn_date(4), "1940-10-06");
}

#[test]
fn aggressive_baseline_always_answers_legally() {
    let content = content();
    for seed in 0..6u8 {
        let played = play_with(&content, seed, seed, &[Side::Axis, Side::Commonwealth]);
        assert_eq!(
            played.rejections, 0,
            "seed {seed}: the baseline produced an illegal answer"
        );
    }
}
