//! Controlled Engineering component clocks, never authored setup or RP entitlement.
//! All source controls below are uncompiled/unrun until separate admission.
use super::*;
use cna_core::decision::{ActionSchema, DecisionResponse};
use cna_core::event::EngineEvent;
use cna_core::visibility::Perspective;
use cna_protocol::GameEvent;
use serde_json::{Value, json};

fn whole(game: &Game<Cna>) -> Vec<u8> {
    serde_json::to_vec(game).unwrap()
}

fn checkpoint(game: &Game<Cna>) -> Game<Cna> {
    serde_json::from_slice(&whole(game)).unwrap()
}

fn first_open_component() -> Game<Cna> {
    let mut value = game(1);
    value.state.cursor.game_turn = content().bounds.start_gt;
    value.state.cursor.block = Block::OpStage;
    value.state.cursor.half = None;
    value.state.cursor.index = 0;
    value.state.cursor.entered = false;
    value.state.turn.initiative = content().scenario.initiative.gt1;
    assert!(
        value
            .state
            .land
            .replacements
            .sides
            .iter()
            .all(|s| s.counters.is_empty())
    );
    // Only the starting public clock/closed setup flag is controlled by game().
    // No TOE, morale, role, reserve, stock, replacement order/counter or CP is added.
    value
}

fn schema_answer(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => {
            json!(options.first().expect("mandatory declared choice").id)
        }
        ActionSchema::Unit { among } => json!(among.first().expect("mandatory declared unit")),
        ActionSchema::Integer { min, .. } => json!(min),
        ActionSchema::Bool => json!(false),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .filter(|f| !f.optional)
                .map(|f| (f.name.clone(), schema_answer(&f.schema)))
                .collect(),
        ),
        ActionSchema::List { min: 0, .. } => json!([]),
        other => panic!("no lawful mandatory component answer for {other:?}"),
    }
}

fn next_command(value: &Game<Cna>, serial: usize) -> Command {
    let Some(request) = Cna::dev()
        .pending(content(), &value.state)
        .into_iter()
        .next()
    else {
        return Command::Advance;
    };
    // Declared passes first. Mandatory source-conserving arrival/attrition answers
    // reuse public procedures; unknown/error/unsupported results are never skipped.
    let action = if request.space.pass.is_some() {
        Value::Null
    } else if let Some(answer) = crate::baseline::arrival_orders(content(), &value.state, &request)
    {
        answer
    } else if request.kind == crate::logistics::attrition::KIND {
        crate::logistics::attrition::baseline(content(), &value.state, request.seat.side)
            .expect("actual conserved mandatory attrition answer")
    } else {
        schema_answer(&request.space.schema)
    };
    request
        .space
        .check(&action)
        .expect("answer uses declared space");
    Command::Respond(DecisionResponse {
        decision_id: request.id,
        seat: request.seat,
        controller_epoch: 1,
        decision_revision: request.revision,
        idempotency_key: format!("engineering-stream-{serial}"),
        action,
        public_explanation: None,
    })
}

fn visible_frame(value: &Game<Cna>, prior: &[EngineEvent], p: Perspective) -> Value {
    let rules = Cna::dev();
    let pending: Vec<_> = rules
        .pending(content(), &value.state)
        .into_iter()
        .filter(|r| match p {
            Perspective::Seat(seat) => r.seat == seat,
            Perspective::Side(side) => r.seat.side == side,
            Perspective::Operator => true,
        })
        .collect();
    let events: Vec<_> = prior
        .iter()
        .filter(|event| p.can_see(&event.audience))
        .collect();
    json!({"perspective":p, "clock":rules.clock(content(), &value.state),
        "events":events, "view":rules.view(content(), &value.state, p),
        "observe":rules.observe(content(), &value.state, p), "pending":pending})
}

fn accepted_with_checkpoint_retry(
    value: &Game<Cna>,
    command: &Command,
) -> cna_core::engine::Transition<Cna> {
    let before = whole(value);
    let saved = checkpoint(value);
    let result = evaluate(&Cna::dev(), content(), value, command).unwrap_or_else(|error| {
        panic!(
            "component {} {command:?}: {error:?}",
            value.state.cursor.anchor()
        )
    });
    let recovered = evaluate(&Cna::dev(), content(), &saved, command).unwrap();
    let retried_from_original = evaluate(&Cna::dev(), content(), value, command).unwrap();
    for replay in [recovered, retried_from_original] {
        assert_eq!(whole(&replay.game), whole(&result.game));
        assert_eq!(replay.events, result.events);
        assert_eq!(replay.progress, result.progress);
    }
    assert_eq!(whole(value), before);
    if let Command::Respond(response) = command {
        let committed = whole(&result.game);
        assert_eq!(
            evaluate(&Cna::dev(), content(), &result.game, command).unwrap_err(),
            Rejection::UnknownDecision {
                decision_id: response.decision_id.clone()
            }
        );
        assert_eq!(whole(&result.game), committed);
    }
    if !result.game.state.decisions.pending.is_empty() {
        let waiting = evaluate(&Cna::dev(), content(), &result.game, &Command::Advance).unwrap();
        assert_eq!(whole(&waiting.game), whole(&result.game));
        assert!(waiting.events.is_empty());
    }
    result
}

/// Cases: land:3.6, land:7.16, land:17.33, land:20.43, land:24.11, land:24.12, airlog:35.11
#[test]
#[ignore = "slow: complete thirteen-perspective paired OpStage/Post/next-opening stream and checkpoint retries"]
fn component_public_empty_first_open_through_post_next_open_has_all_thirteen_ordered_streams() {
    let mut a = first_open_component();
    let mut b = checkpoint(&a);
    // Independent pair: c varies only a real canonical SGSU's private history.
    // b retains the original, separate Engineering history variation below.
    let mut c = checkpoint(&a);
    let start = a.state.cursor.game_turn;
    assert!(
        content().bounds.end_gt > start,
        "source scenario must include next turn"
    );
    let mut prior_a = Vec::new();
    let mut prior_b = Vec::new();
    let mut prior_c = Vec::new();
    let perspectives: Vec<_> = Perspective::all().collect();
    assert_eq!(perspectives.len(), 13);
    let mut closed = BTreeSet::new();
    let mut saw_post = false;
    let mut responses = 0usize;
    let mut private_pair_added = false;
    let mut sgsu_id = None;
    let mut saw_differing_current = false;
    let mut saw_differing_retained = false;
    for serial in 0..4096 {
        for &p in &perspectives {
            assert_eq!(
                visible_frame(&a, &prior_a, p),
                visible_frame(&b, &prior_b, p),
                "before command {serial} {p}"
            );
            assert_eq!(
                visible_frame(&a, &prior_a, p),
                visible_frame(&c, &prior_c, p),
                "SGSU before command {serial} {p}"
            );
        }
        let command = next_command(&a, serial);
        if matches!(command, Command::Respond(_)) {
            responses += 1;
        }
        let left = accepted_with_checkpoint_retry(&a, &command);
        let right = accepted_with_checkpoint_retry(&b, &command);
        let sgsu = accepted_with_checkpoint_retry(&c, &command);
        assert_eq!(left.progress, right.progress);
        assert_eq!(
            left.events, right.events,
            "complete raw phase/event order, including Operator"
        );
        assert_eq!(left.progress, sgsu.progress);
        assert_eq!(
            left.events, sgsu.events,
            "SGSU complete raw phase/event order, including Operator"
        );
        for event in &left.events {
            if let GameEvent::PhaseChanged { clock } = &event.event {
                saw_post |= clock.game_turn == start
                    && clock.op_stage.is_none()
                    && matches!(
                        clock.stage.as_str(),
                        "strategic_air_recovery" | "end_of_turn"
                    );
            }
        }
        prior_a.extend(left.events);
        prior_b.extend(right.events);
        prior_c.extend(sgsu.events);
        a = left.game;
        b = right.game;
        c = sgsu.game;
        assert_eq!(a.rng, b.rng);
        assert_eq!(
            serde_json::to_value(&a.state.land).unwrap(),
            serde_json::to_value(&b.state.land).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&a.state.logistics).unwrap(),
            serde_json::to_value(&b.state.logistics).unwrap()
        );
        assert!(
            a.state
                .land
                .replacements
                .sides
                .iter()
                .all(|s| s.counters.is_empty())
        );
        assert!(
            b.state
                .land
                .replacements
                .sides
                .iter()
                .all(|s| s.counters.is_empty())
        );
        assert_eq!(a.rng, c.rng);
        assert_eq!(
            serde_json::to_value(&a.state.land).unwrap(),
            serde_json::to_value(&c.state.land).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&a.state.logistics).unwrap(),
            serde_json::to_value(&c.state.logistics).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&a.state.engineering).unwrap(),
            serde_json::to_value(&c.state.engineering).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&a.state.air.runtime.sgsus).unwrap(),
            serde_json::to_value(&c.state.air.runtime.sgsus).unwrap()
        );
        assert!(
            c.state
                .land
                .replacements
                .sides
                .iter()
                .all(|s| s.counters.is_empty())
        );
        if !private_pair_added
            && a.state.engineering.observed_opening
                == Some(ActivityStage {
                    game_turn: start,
                    op_stage: 1,
                })
        {
            // Explicit private-record variation, never a CP expenditure, pin action,
            // training entitlement or altered public world state.
            let entry = b
                .state
                .engineering
                .activity
                .values_mut()
                .next()
                .expect("observed physical source unit");
            entry.gross_cp_quarters = 4;
            entry.departed = true;
            entry.pinned = true;
            private_pair_added = true;
        }
        if sgsu_id.is_none()
            && a.state.engineering.observed_opening
                == Some(ActivityStage {
                    game_turn: start,
                    op_stage: 1,
                })
        {
            let (id, unit) = c
                .state
                .air
                .runtime
                .sgsus
                .iter()
                .next()
                .expect("existing canonical source SGSU");
            let id = id.clone();
            let owner = unit.side().expect("canonical SGSU owner");
            let observed = c
                .state
                .air
                .runtime
                .sgsu_activity
                .current
                .as_mut()
                .expect("actual public opening history");
            assert_eq!(
                observed.stage,
                SgsuOpStage {
                    game_turn: start,
                    op_stage: 1
                }
            );
            assert!(observed.observed_opening && !observed.closed);
            let entry = observed
                .entries
                .get_mut(&id)
                .expect("real opening recorded canonical id");
            assert_eq!(entry.owner, owner);
            assert_eq!(
                &*entry,
                a.state
                    .air
                    .runtime
                    .sgsu_activity
                    .current
                    .as_ref()
                    .unwrap()
                    .entries
                    .get(&id)
                    .unwrap()
            );
            // Private observations only: no CP expenditure, actual movement/pin,
            // producer-coverage claim, new id, candidate, inventory or entitlement.
            entry.gross_cp_quarters = entry.gross_cp_quarters.checked_add(4).unwrap();
            entry.departed = true;
            entry.pinned = true;
            sgsu_id = Some(id);
        }
        if let Some(id) = &sgsu_id {
            let interval = SgsuOpStage {
                game_turn: start,
                op_stage: 1,
            };
            if let Some(observed) = &c.state.air.runtime.sgsu_activity.current {
                if observed.stage == interval && !observed.closed {
                    let base = a.state.air.runtime.sgsu_activity.current.as_ref().unwrap();
                    assert_eq!(base.stage, observed.stage);
                    let left = base.entries.get(id).unwrap();
                    let right = observed.entries.get(id).unwrap();
                    assert_ne!(left, right, "active private current pair must differ");
                    assert_eq!(left.owner, right.owner);
                    assert_eq!(left.opening_location, right.opening_location);
                    assert_eq!(left.closing_location, right.closing_location);
                    assert_eq!(right.gross_cp_quarters, left.gross_cp_quarters + 4);
                    assert!(right.departed && right.pinned);
                    assert_eq!(
                        whole(&checkpoint(&c)),
                        whole(&c),
                        "active SGSU history survives complete checkpoint recovery"
                    );
                    saw_differing_current = true;
                }
            }
            if let Some(retained) = &c.state.air.runtime.sgsu_activity.previous_closed {
                if retained.stage == interval {
                    let base = a
                        .state
                        .air
                        .runtime
                        .sgsu_activity
                        .previous_closed
                        .as_ref()
                        .unwrap();
                    assert_eq!(base.stage, retained.stage);
                    assert!(base.closed && retained.closed && retained.observed_opening);
                    let left = base.entries.get(id).unwrap();
                    let right = retained.entries.get(id).unwrap();
                    assert_ne!(
                        left, right,
                        "actual close must retain the differing SGSU observations"
                    );
                    assert_eq!(left.owner, right.owner);
                    assert_eq!(left.opening_location, right.opening_location);
                    assert_eq!(left.closing_location, right.closing_location);
                    assert_eq!(right.gross_cp_quarters, left.gross_cp_quarters + 4);
                    assert!(right.departed && right.pinned);
                    assert_eq!(
                        whole(&checkpoint(&c)),
                        whole(&c),
                        "retained SGSU history survives complete checkpoint recovery"
                    );
                    saw_differing_retained = true;
                }
            }
        }
        if let Some(record) = &a.state.engineering.closed_activity {
            if record.stage.game_turn == start {
                closed.insert(record.stage.op_stage);
            }
        }
        for &p in &perspectives {
            assert_eq!(
                visible_frame(&a, &prior_a, p),
                visible_frame(&b, &prior_b, p),
                "after command {serial} {p}"
            );
            assert_eq!(
                visible_frame(&a, &prior_a, p),
                visible_frame(&c, &prior_c, p),
                "SGSU after command {serial} {p}"
            );
        }
        if a.state.engineering.observed_opening
            == Some(ActivityStage {
                game_turn: start + 1,
                op_stage: 1,
            })
        {
            assert!(private_pair_added && responses > 0 && saw_post);
            assert_eq!(closed, [1u8, 2, 3].into_iter().collect());
            assert_eq!(prior_a, prior_b);
            assert!(
                saw_differing_current && saw_differing_retained,
                "SGSU current AND actual retained history variation required"
            );
            assert_eq!(prior_a, prior_c);
            assert_eq!(
                c.state.engineering.observed_opening,
                a.state.engineering.observed_opening
            );
            assert_eq!(
                c.state
                    .air
                    .runtime
                    .sgsu_activity
                    .current
                    .as_ref()
                    .unwrap()
                    .stage,
                SgsuOpStage {
                    game_turn: start + 1,
                    op_stage: 1
                }
            );
            let retained = a.state.engineering.closed_activity.as_ref().unwrap();
            assert_eq!(
                retained.stage,
                ActivityStage {
                    game_turn: start,
                    op_stage: 3
                }
            );
            assert_eq!(
                a.state
                    .air
                    .runtime
                    .sgsu_activity
                    .current
                    .as_ref()
                    .unwrap()
                    .stage,
                SgsuOpStage {
                    game_turn: start + 1,
                    op_stage: 1
                }
            );
            return;
        }
    }
    panic!("bounded public component stream did not reach next true opening");
}

/// Cases: land:23.22, land:24.11, land:24.22, land:24.23, airlog:35.11
#[test]
#[ignore = "slow: real response/Advance legacy preclose recovery across a complete controlled OpStage"]
fn component_midstage_legacy_recovery_never_fabricates_opening_or_quiet_sgsu_coverage() {
    let opened = evaluate(
        &Cna::dev(),
        content(),
        &first_open_component(),
        &Command::Advance,
    )
    .unwrap();
    let stage = opened.game.state.engineering.observed_opening.unwrap();
    assert!(!opened.game.state.decisions.pending.is_empty());
    let mut legacy = checkpoint(&opened.game);
    legacy.state.engineering.observed_opening = None;
    legacy.state.engineering.activity.clear();
    legacy.state.engineering.closed_activity = None;
    legacy.state.air.runtime.sgsu_activity = Default::default();
    // Missing persisted fields at an ENTERED midstage clock, not retrospective
    // public opening. Never alter canonical units, resources, SGSU position or RP.
    let saved = checkpoint(&legacy);
    assert_eq!(whole(&saved), whole(&legacy));
    for serial in 0..2048 {
        if let Some(closed) = &legacy.state.engineering.closed_activity {
            if closed.stage == stage {
                assert_eq!(closed.observed_opening, None);
                assert!(
                    closed
                        .entries
                        .values()
                        .all(|e| e.activity.opening_location.is_none())
                );
                let selected = SgsuOpStage {
                    game_turn: stage.game_turn,
                    op_stage: stage.op_stage,
                };
                let air = legacy
                    .state
                    .air
                    .runtime
                    .sgsu_activity
                    .previous_closed
                    .as_ref()
                    .unwrap();
                assert_eq!(air.stage, selected);
                assert!(!air.observed_opening);
                for (id, unit) in &legacy.state.air.runtime.sgsus {
                    let evidence = activity::stage_evidence(
                        content(),
                        &legacy.state,
                        unit.side().unwrap(),
                        id,
                        selected,
                    )
                    .unwrap();
                    let activity::SgsuEvidence::Unknown { reason, recorded } = evidence else {
                        panic!("missing producers cannot certify a complete quiet interval")
                    };
                    assert_eq!(
                        reason,
                        activity::SgsuEvidenceGap::IncompleteProducerCoverage
                    );
                    assert!(recorded.is_some_and(|entry| entry.opening_location.is_none()));
                }
                return;
            }
        }
        if legacy.state.cursor.op_stage == Some(stage.op_stage) {
            assert_eq!(legacy.state.engineering.observed_opening, None);
        }
        let command = next_command(&legacy, serial);
        legacy = accepted_with_checkpoint_retry(&legacy, &command).game;
    }
    panic!("legacy component did not close its entered interval");
}

fn rejection_pure(value: &Game<Cna>, command: &Command, expected: Rejection) {
    let before = whole(value);
    let saved = checkpoint(value);
    let prior = evaluate(
        &Cna::dev(),
        content(),
        &first_open_component(),
        &Command::Advance,
    )
    .unwrap()
    .events;
    let prior_bytes = serde_json::to_vec(&prior).unwrap();
    for original in [value, &saved, value] {
        assert_eq!(
            evaluate(&Cna::dev(), content(), original, command).unwrap_err(),
            expected
        );
        assert_eq!(whole(original), before);
        assert_eq!(original.rng, value.rng);
    }
    assert_eq!(serde_json::to_vec(&prior).unwrap(), prior_bytes);
}

/// Cases: land:3.6, land:17.33, land:20.41, land:20.43
#[test]
fn empty_oob_duplicate_side_errors_at_real_public_open_and_respond_restore_whole_advance() {
    use crate::land::replacements::SideReplacements;
    for anchor in [
        "opstage.organization.reorganization",
        "opstage.organization.training",
    ] {
        let mut original = first_open_component();
        original.state.cursor.index = crate::seq::OPSTAGE
            .iter()
            .position(|step| step.anchor == anchor)
            .unwrap();
        // Explicit component entry clock; no replacement counter or candidate.
        let mut broken = checkpoint(&original);
        broken.state.land.replacements.sides = vec![
            SideReplacements::new(Side::Axis),
            SideReplacements::new(Side::Axis),
            SideReplacements::new(Side::Commonwealth),
        ];
        let expected = Rejection::Engine(EngineError::Invariant {
            detail: "replacements: duplicate replacement side".into(),
        });
        rejection_pure(&broken, &Command::Advance, expected.clone());
        let opened = evaluate(&Cna::dev(), content(), &original, &Command::Advance)
            .unwrap()
            .game;
        assert_eq!(opened.state.decisions.pending.len(), 6);
        assert!(
            opened
                .state
                .land
                .replacements
                .sides
                .iter()
                .all(|side| side.counters.is_empty())
        );
        let command = next_command(&opened, 0);
        let Command::Respond(response) = &command else {
            panic!("public opened request required")
        };
        assert!(response.action.is_null());
        let mut broken = checkpoint(&opened);
        let duplicate = broken
            .state
            .land
            .replacements
            .sides
            .iter()
            .find(|s| s.side == response.seat.side)
            .unwrap()
            .clone();
        assert!(duplicate.counters.is_empty());
        broken.state.land.replacements.sides.push(duplicate);
        rejection_pure(&broken, &command, expected);
    }
}

/// Cases: land:17.33, land:20.43, land:24.11, land:24.12
#[test]
fn empty_oob_stage_callback_is_exactly_byte_pure_and_has_no_invented_failure_cell() {
    for invalid_label in [false, true] {
        let mut value = game(1);
        assert!(
            value
                .state
                .land
                .replacements
                .sides
                .iter()
                .all(|s| s.counters.is_empty())
        );
        if invalid_label {
            value.state.cursor.op_stage = None;
            value.state.cursor.index = usize::MAX;
        }
        let before = whole(&value);
        crate::land::replacements::finish_training_stage(content(), &mut value.state).unwrap();
        assert_eq!(whole(&value), before);
        crate::land::replacements::finish_training_stage(content(), &mut value.state).unwrap();
        assert_eq!(whole(&value), before);
    }
    // Source448-462 early Ok makes an INSIDE-credit error structurally unreachable
    // with no candidates. Air/Engineering authenticate the actual shared boundary.
    // Nonempty real-credit/later-failure proof remains REQUIRED first delivery c/d.
}

fn entry(value: &Game<Cna>) -> (crate::air::sgsu::SgsuId, activity::SgsuActivityEntry) {
    let (id, unit) = value
        .state
        .air
        .runtime
        .sgsus
        .iter()
        .next()
        .expect("canonical source SGSU");
    (
        id.clone(),
        activity::SgsuActivityEntry {
            owner: unit.side().unwrap(),
            opening_location: None,
            closing_location: None,
            gross_cp_quarters: 0,
            departed: false,
            pinned: false,
        },
    )
}

/// Cases: land:24.11, land:24.12, land:24.22, land:24.23, airlog:35.11
#[test]
fn reachable_air_and_engineering_oldtail_corruptions_restore_complete_public_advance() {
    let mut cases = Vec::new();
    for (gt, op) in [(0, Some(1)), (1, Some(0)), (1, Some(4))] {
        let mut value = game(1);
        value.state.cursor.game_turn = gt;
        value.state.cursor.op_stage = op;
        cases.push((value, "air SGSU activity: invalid Operations Stage label"));
    }
    let mut missing = game(1);
    missing.state.cursor.op_stage = None;
    cases.push((missing, "old-stage boundary has no Operations Stage"));
    let mut future = game(1);
    future.state.air.runtime.sgsu_activity.current = Some(air_closed(2));
    cases.push((future, "air SGSU activity: future recorded interval"));
    let mut unclosed = game(2);
    let mut prior = air_closed(1);
    prior.closed = false;
    unclosed.state.air.runtime.sgsu_activity.previous_closed = Some(prior);
    cases.push((
        unclosed,
        "air SGSU activity: retained interval is not closed",
    ));
    let mut unmatched = game(1);
    unmatched.state.air.runtime.sgsu_activity.current = Some(air_closed(1));
    cases.push((
        unmatched,
        "air SGSU activity: closed current interval differs from immutable retained interval",
    ));
    let mut order = game(3);
    let mut current = air_closed(1);
    current.closed = false;
    order.state.air.runtime.sgsu_activity.current = Some(current);
    order.state.air.runtime.sgsu_activity.previous_closed = Some(air_closed(2));
    cases.push((
        order,
        "air SGSU activity: retained interval is later than current history",
    ));
    let mut uninitialized = game(1);
    assert!(!uninitialized.state.air.runtime.sgsus.is_empty());
    uninitialized.state.air.runtime.bases_initialized = false;
    cases.push((
        uninitialized,
        "air SGSU activity: uninitialized canonical base maps contain records",
    ));
    let mut import = game(1);
    import.state.air.runtime.initialized = false;
    cases.push((
        import,
        "air inventory: inventory and bases must be imported",
    ));
    let mut owner = game(1);
    let (id, mut item) = entry(&owner);
    item.owner = item.owner.opponent();
    let mut current = air_closed(1);
    current.closed = false;
    current.entries.insert(id, item);
    owner.state.air.runtime.sgsu_activity.current = Some(current);
    cases.push((owner, "air SGSU activity: historical owner was rewritten"));
    for fault in 0..6 {
        let mut value = game(1);
        let (mut id, mut item) = entry(&value);
        let mut current = air_closed(1);
        current.closed = false;
        match fault {
            0 => item.gross_cp_quarters = -1,
            1 => item.opening_location = Some(Location::NotArrived),
            2 => {
                item.closing_location = Some(
                    value.state.air.runtime.sgsus[&id]
                        .location(content(), &value.state.air.runtime.facilities)
                        .unwrap(),
                )
            }
            3 => {
                current.closed = true;
            }
            4 => id.0.clear(),
            _ => item.closing_location = Some(Location::NotArrived),
        }
        current.entries.insert(id, item);
        if fault == 3 {
            value.state.air.runtime.sgsu_activity.previous_closed = Some(current.clone());
        }
        value.state.air.runtime.sgsu_activity.current = Some(current);
        cases.push((
            value,
            match fault {
                0 => "air SGSU activity: corrupt selected activity",
                1 => "air SGSU activity: invalid physical historical observation",
                2 => "air SGSU activity: unclosed interval contains a final observation",
                3 => "air SGSU activity: same-stage closure would rewrite history",
                4 => "air SGSU activity: corrupt selected activity",
                _ => "air SGSU activity: invalid physical historical observation",
            },
        ));
    }
    let mut global = game(1);
    global.state.engineering.observed_opening = Some(ActivityStage {
        game_turn: 2,
        op_stage: 1,
    });
    cases.push((global, "engineering closure has invalid opening history"));
    for future in [false, true] {
        let mut value = game(1);
        let id = value.state.land.units.keys().next().unwrap().clone();
        value.state.engineering.activity.insert(
            id,
            UnitActivity {
                stage: ActivityStage {
                    game_turn: 1,
                    op_stage: if future { 2 } else { 1 },
                },
                opening_location: None,
                gross_cp_quarters: if future { 0 } else { -1 },
                departed: false,
                pinned: false,
            },
        );
        cases.push((value, "engineering closure has corrupt selected activity"));
    }
    let mut physical = game(1);
    let id = physical.state.land.units.keys().next().unwrap().clone();
    physical.state.engineering.activity.insert(
        id,
        UnitActivity {
            stage: ActivityStage {
                game_turn: 1,
                op_stage: 1,
            },
            opening_location: Some(Location::NotArrived),
            gross_cp_quarters: 0,
            departed: false,
            pinned: false,
        },
    );
    cases.push((
        physical,
        "air SGSU activity: invalid physical historical observation",
    ));
    let mut identity = game(1);
    identity.state.engineering.activity.insert(
        cna_core::ids::UnitId::new(""),
        UnitActivity {
            stage: ActivityStage {
                game_turn: 1,
                op_stage: 1,
            },
            opening_location: None,
            gross_cp_quarters: 0,
            departed: false,
            pinned: false,
        },
    );
    cases.push((
        identity,
        "engineering closure has corrupt selected activity",
    ));
    let mut duplicate = game(1);
    close_pair(&mut duplicate.state).unwrap();
    duplicate.state.engineering.observed_opening = Some(ActivityStage {
        game_turn: 1,
        op_stage: 1,
    });
    cases.push((
        duplicate,
        "engineering same-stage closure would rewrite history",
    ));
    let mut required = game(2);
    let mut prior = land_closed(1);
    // Orphan authority is an explicit corrupt retained ID, never a created project/RP.
    prior
        .required_facility_projects
        .insert(serde_json::from_value(json!(1u64)).unwrap());
    required.state.engineering.closed_activity = Some(prior);
    cases.push((
        required,
        "engineering facility project: retained interval references an unknown required project",
    ));
    let mut label = game(2);
    let mut prior = land_closed(1);
    prior.stage.game_turn = 0;
    label.state.engineering.closed_activity = Some(prior);
    cases.push((label, "engineering retirement has an invalid prior label"));
    let mut retained = game(2);
    let mut prior = land_closed(1);
    prior.observed_opening = Some(ActivityStage {
        game_turn: 1,
        op_stage: 2,
    });
    retained.state.engineering.closed_activity = Some(prior);
    cases.push((
        retained,
        "engineering retained interval has invalid stage metadata",
    ));
    for (value, detail) in cases {
        assert!(
            value
                .state
                .land
                .replacements
                .sides
                .iter()
                .all(|s| s.counters.is_empty())
        );
        rejection_pure(
            &value,
            &Command::Advance,
            Rejection::Engine(EngineError::Invariant {
                detail: detail.into(),
            }),
        );
    }
    // Genuine unmatched old interval is a typed source stop, not a fabricated quiet history.
    let mut older = game(2);
    let mut current = air_closed(1);
    current.closed = false;
    older.state.air.runtime.sgsu_activity.current = Some(current);
    rejection_pure(
        &older,
        &Command::Advance,
        Rejection::Engine(EngineError::Unsupported {
            case: "land:24.11".into(),
            detail: "unclosed older SGSU interval would be discarded".into(),
        }),
    );
    for mismatch in [false, true] {
        let mut value = game(3);
        value.state.air.runtime.sgsu_activity.previous_closed = Some(air_closed(2));
        if mismatch {
            value.state.engineering.closed_activity = Some(land_closed(1));
        }
        rejection_pure(
            &value,
            &Command::Advance,
            Rejection::Engine(EngineError::Unsupported {
                case: "land:24.11".into(),
                detail: if mismatch {
                    "legacy retained intervals have unresolved pairing"
                } else {
                    "retained Air interval has no authenticated Engineering dependency record"
                }
                .into(),
            }),
        );
    }
}

/// Cases: land:24.11, land:24.12, land:24.22, land:24.23
#[test]
fn empty_duplicate_close_is_whole_game_noop_and_direct_mismatched_label_is_exact() {
    let mut value = game(1);
    let before = whole(&value);
    assert_eq!(
        activity::capture_sgsu_stage(
            content(),
            &mut value.state,
            SgsuOpStage {
                game_turn: 1,
                op_stage: 2
            }
        ),
        Err(EngineError::Invariant {
            detail: "air SGSU activity: old-tail stage mismatch".into()
        })
    );
    assert_eq!(whole(&value), before);
    close_pair(&mut value.state).unwrap();
    let once = whole(&value);
    assert!(
        value
            .state
            .engineering
            .closed_activity
            .as_ref()
            .unwrap()
            .required_facility_projects
            .is_empty()
    );
    close_pair(&mut value.state).unwrap();
    assert_eq!(whole(&value), once);
}

/// Cases: land:24.11, land:24.12, land:24.22, land:24.23
#[test]
fn checked_tail_and_true_open_label_guards_preserve_whole_component_and_rng() {
    for fault in 0..6 {
        let mut value = game(1);
        match fault {
            0 => value.state.cursor.block = Block::Pre,
            1 => value.state.cursor.half = Some(Half::A),
            2 => value.state.cursor.index = 0,
            3 => value.state.cursor.index = usize::MAX,
            4 => value.state.cursor.entered = false,
            _ => {
                let opened = evaluate(
                    &Cna::dev(),
                    content(),
                    &first_open_component(),
                    &Command::Advance,
                )
                .unwrap();
                assert!(!opened.game.state.decisions.pending.is_empty());
                value.state.decisions.pending = opened.game.state.decisions.pending;
            }
        }
        let before = whole(&value);
        let expected = EngineError::Invariant {
            detail: "engineering closure outside the completed old-stage tail".into(),
        };
        assert_eq!(
            activity::capture_sgsu_stage(
                content(),
                &mut value.state,
                SgsuOpStage {
                    game_turn: 1,
                    op_stage: 1
                }
            ),
            Err(expected.clone())
        );
        assert_eq!(whole(&value), before);
        assert_eq!(close_stage(&mut value.state), Err(expected));
        assert_eq!(whole(&value), before);
    }
    for (gt, op, detail) in [
        (
            0,
            Some(1),
            "air SGSU activity: invalid Operations Stage label",
        ),
        (
            1,
            Some(4),
            "air SGSU activity: invalid Operations Stage label",
        ),
        (1, None, "air SGSU activity: missing Operations Stage"),
    ] {
        let mut value = first_open_component();
        value.state.cursor.game_turn = gt;
        value.state.cursor.op_stage = op;
        rejection_pure(
            &value,
            &Command::Advance,
            Rejection::Engine(EngineError::Invariant {
                detail: detail.into(),
            }),
        );
    }
}
