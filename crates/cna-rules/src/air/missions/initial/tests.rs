use super::*;
use crate::{Cna, air::{designation, inventory, missions}, seq::Block, testkit::{assert_action_indistinguishable, assert_actions_indistinguishable, assert_indistinguishable}};
use cna_core::{decision::DecisionResponse, dice::CampaignRng, engine::{Command, Game, Ruleset, evaluate}};
use serde_json::{Value, json};
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}

fn first(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => json!(options.first().expect("actual setup choice").id),
        ActionSchema::Unit { among } => json!(among.first().expect("actual setup unit")),
        ActionSchema::Integer { max, .. } => json!(max),
        ActionSchema::Bool => json!(false),
        ActionSchema::Record { fields } => Value::Object(fields.iter().map(|f| (f.name.clone(), first(&f.schema))).collect()),
        ActionSchema::List { item, min, .. } => Value::Array((0..*min).map(|_| first(item)).collect()),
        _ => panic!("unexpected actual setup schema"),
    }
}

fn response(game: &Game<Cna>, side: Side, action: Value) -> Command {
    let p = game.state.decisions.pending.iter().find(|p| p.seat.side == side).unwrap();
    Command::Respond(DecisionResponse {
        decision_id: p.id.clone(), seat: p.seat, controller_epoch: 1,
        decision_revision: p.revision, idempotency_key: p.id.to_string(),
        action, public_explanation: None,
    })
}

fn respond(game: &Game<Cna>, side: Side, action: Value) -> Game<Cna> {
    evaluate(&Cna::dev(), content(), game, &response(game, side, action)).unwrap().game
}

fn bytes(game: &Game<Cna>) -> Vec<u8> { serde_json::to_vec(game).unwrap() }

// Genuine source setup/Respond/final-close: no fabricated started/closed flags,
// authority token, original aircraft counts, ledger or preparation map.
fn fresh_closed() -> Game<Cna> {
    static FRESH: OnceLock<Game<Cna>> = OnceLock::new();
    FRESH.get_or_init(|| {
        let mut game = Game { state: State::new(content()).unwrap(), rng: CampaignRng::from_seed([5; 32]).state() };
        for _ in 0..500 {
            if game.state.setup.closed {
                assert!(!game.state.air.runtime.preparation.initial_choices.is_empty());
                assert!(game.state.air.runtime.preparation.initial_choices.values().all(|c| *c == InitialChoice::Unselected));
                assert!(game.state.air.runtime.preparation.current.values().all(|c| *c == CurrentPreparation::InitialUnspecified));
                return game;
            }
            if let Some(p) = game.state.decisions.pending.first().cloned() {
                let command = Command::Respond(DecisionResponse {
                    decision_id: p.id.clone(), seat: p.seat, controller_epoch: 1,
                    decision_revision: p.revision, idempotency_key: p.id.to_string(),
                    action: first(&p.space.schema), public_explanation: None,
                });
                game = evaluate(&Cna::dev(), content(), &game, &command).unwrap().game;
            } else {
                game = evaluate(&Cna::dev(), content(), &game, &Command::Advance).unwrap().game;
            }
        }
        panic!("actual fresh setup failed to close");
    }).clone()
}

// Component clock entry only: setup provenance and both procedures are real
// public flows, but this does not claim a whole intervening campaign trajectory.
fn at(game: &mut Game<Cna>, block: Block, anchor: &str, op: Option<u8>) {
    assert!(game.state.decisions.pending.is_empty());
    game.state.cursor.block = block;
    game.state.cursor.index = block.steps().iter().position(|step| step.anchor == anchor).unwrap();
    game.state.cursor.op_stage = op;
    game.state.cursor.entered = false;
}

fn opened() -> Game<Cna> {
    static OPENED: OnceLock<Game<Cna>> = OnceLock::new();
    OPENED.get_or_init(|| {
        let mut game = fresh_closed();
        assert!(game.state.decisions.pending.iter().all(|p| p.kind == designation::KIND));
        assert_eq!(game.state.decisions.pending.len(), 2);
        for side in Side::ALL { game = respond(&game, side, Value::Null); }
        // Finish the genuine designation answers through their actual dispatcher,
        // then intercept only the clock before the declaration component.
        let mut rng = CampaignRng::from_state(&game.rng);
        let mut events = Vec::new();
        Cna::dev().finish_step(content(), &mut game.state, &mut cna_core::engine::Cx { rng: &mut rng, events: &mut events }).unwrap();
        assert_eq!(rng.state(), game.rng);
        assert!(game.state.air.runtime.designation.finished);
        at(&mut game, Block::OpStage, missions::ANCHOR, Some(1));
        let result = evaluate(&Cna::dev(), content(), &game, &Command::Advance).unwrap();
        assert_eq!(result.game.rng, game.rng);
        assert_eq!(result.game.state.decisions.pending.len(), 2);
        result.game
    }).clone()
}

fn fighter(game: &Game<Cna>, side: Side) -> (PlaneId, usize) {
    missions::own_planes(&game.state, side).into_iter().find_map(|id| {
        let aircraft = &content().units.aircraft[&game.state.air.runtime.aircraft[&id].aircraft];
        aircraft.modes.iter().position(|m| m.missions.get("s").is_some_and(|c| matches!(c.as_str(), "day" | "night"))).map(|mode| (id, mode))
    }).expect("real initial aircraft with source scramble capability")
}

fn action(game: &Game<Cna>, side: Side, choice: Value) -> (PlaneId, Value) {
    let (id, mode) = fighter(game, side);
    (id.clone(), json!([{"mission":"scramble","light":"day","target":{"scramble_reserve":true},
        "planes":[{"plane":id,"mode":mode,"initial_choice":choice}]}]))
}

fn load(guns: bool) -> Value { json!({"decision":"load","guns":guns,"ordnance":{"kind":"none"}}) }

fn answered(game: &Game<Cna>, side: Side, choice: Value) -> (PlaneId, Game<Cna>) {
    let (id, plan) = action(game, side, choice);
    let a = respond(game, side, plan);
    let b = respond(&a, side.opponent(), Value::Null);
    (id, b)
}

/// Cases: scen:59.2, scen:59.32, scen:59.36, airlog:39.12, airlog:39.16
/// Interpretations: interp:air-0018
#[test]
fn genuine_setup_and_public_declaration_commit_once_in_first_stage_without_payment_or_flight() {
    let game = opened();
    assert_eq!(game.state.cursor.op_stage, Some(content().scenario.meta.start.opstage));
    let (id, ready) = answered(&game, Side::Axis, load(true));
    assert_eq!(ready.state.air.runtime.preparation, game.state.air.runtime.preparation);
    assert_eq!(ready.rng, game.rng);
    let restored: Game<Cna> = serde_json::from_slice(&bytes(&ready)).unwrap();
    let a = evaluate(&Cna::dev(), content(), &ready, &Command::Advance).unwrap();
    let b = evaluate(&Cna::dev(), content(), &restored, &Command::Advance).unwrap();
    assert_eq!(bytes(&a.game), bytes(&b.game));
    assert_eq!(a.events, b.events);
    assert_eq!(a.game.rng, game.rng);
    assert_eq!(serde_json::to_value(&a.game.state.logistics).unwrap(), serde_json::to_value(&game.state.logistics).unwrap());
    assert_eq!(a.game.state.air.runtime.maintenance, game.state.air.runtime.maintenance);
    assert_eq!(a.game.state.air.runtime.plane_serial, game.state.air.runtime.plane_serial);
    assert_eq!(a.game.state.air.runtime.pilot_serial, game.state.air.runtime.pilot_serial);
    assert_eq!(a.game.state.air.runtime.aircraft, game.state.air.runtime.aircraft);
    assert_eq!(a.game.state.air.runtime.preparation.initial_choices[&id], InitialChoice::Committed);
    let CurrentPreparation::KnownLoad { load } = &a.game.state.air.runtime.preparation.current[&id] else { panic!("missing initial complete load") };
    assert!(load.fuel_available && load.gun_ammunition && load.ordnance.is_none());
    for (other, old) in &game.state.air.runtime.preparation.current {
        if other != &id { assert_eq!(&a.game.state.air.runtime.preparation.current[other], old); }
    }
    let mut once = a.game.clone();
    let before = bytes(&once);
    missions::finish(content(), &mut once.state).unwrap();
    assert_eq!(bytes(&once), before);
    inventory::check(content(), &once.state.air).unwrap();
}

/// Cases: scen:59.32, scen:59.36, airlog:38.41, airlog:39.12
/// Interpretations: interp:air-0018
#[test]
fn explicit_forfeit_and_false_guns_spend_whole_right_but_pass_and_unlisted_preserve_it() {
    for (choice, forfeit) in [(json!({"decision":"forfeit"}), true), (load(false), false)] {
        let game = opened();
        let (id, ready) = answered(&game, Side::Axis, choice);
        let done = evaluate(&Cna::dev(), content(), &ready, &Command::Advance).unwrap().game;
        assert_eq!(done.state.air.runtime.preparation.initial_choices[&id], if forfeit { InitialChoice::Forfeited } else { InitialChoice::Committed });
        if forfeit {
            assert_eq!(done.state.air.runtime.preparation.current[&id], CurrentPreparation::Unprepared);
            assert!(!done.state.air.runtime.aircraft[&id].fuelled);
        } else {
            let CurrentPreparation::KnownLoad { load } = &done.state.air.runtime.preparation.current[&id] else { panic!() };
            assert!(load.fuel_available && !load.gun_ammunition && load.ordnance.is_none());
        }
        assert!(!done.state.air.runtime.aircraft[&id].armed);
        assert_eq!(done.state.air.runtime.aircraft[&id].refitted, game.state.air.runtime.aircraft[&id].refitted);
        for other in game.state.air.runtime.aircraft.keys().filter(|other| *other != &id) {
            assert_eq!(done.state.air.runtime.preparation.initial_choices[other], InitialChoice::Unselected);
        }
        let before = bytes(&done);
        let (_, plan) = action(&done, Side::Axis, load(true));
        let wire: Vec<missions::WireMission> = serde_json::from_value(plan).unwrap();
        assert!(matches!(validate_answer(content(), &done.state, Side::Axis, &wire[0].planes[0]), Err(Rejection::Illegal { .. })));
        assert_eq!(bytes(&done), before);
    }
    let game = opened();
    let a = respond(&respond(&game, Side::Axis, Value::Null), Side::Commonwealth, json!([]));
    let done = evaluate(&Cna::dev(), content(), &a, &Command::Advance).unwrap().game;
    assert_eq!(done.state.air.runtime.preparation, game.state.air.runtime.preparation);
}

/// Cases: scen:59.32, airlog:39.12, land:3.6
/// Interpretations: interp:air-0018
#[test]
fn advertised_optional_null_false_and_own_known_retry_keep_pass_legal_and_source_immutable() {
    let game = opened();
    let request = Cna::dev().pending(content(), &game.state).into_iter().find(|p| p.seat.side == Side::Axis).unwrap();
    let context = request.space.context.as_ref().unwrap();
    let own = missions::own_planes(&game.state, Side::Axis);
    assert_eq!(context["planes"].as_object().unwrap().len(), own.len());
    assert_eq!(context["initial_choices"].as_object().unwrap().len(), own.len());
    for id in own {
        assert_eq!(context["planes"][id.0.as_str()], serde_json::to_value(&game.state.air.runtime.aircraft[&id]).unwrap());
        assert_eq!(context["initial_choices"][id.0.as_str()], serde_json::to_value(InitialChoice::Unselected).unwrap());
    }
    assert!(context["planes"].as_object().unwrap().values().all(|plane| plane["force"] == "axis"));
    request.space.check(&Value::Null).unwrap();
    assert_eq!(crate::baseline::random_orders(content(), &game.state, &request, &mut CampaignRng::from_seed([19;32])), Value::Null);
    let before = bytes(&game);
    for choice in [Value::Null, json!({"decision":"load"}), json!({"decision":"load","guns":null,"ordnance":{"kind":"none"}}), json!({"decision":"forfeit","guns":false})] {
        let (_, plan) = action(&game, Side::Axis, choice);
        request.space.check(&plan).unwrap();
        assert!(matches!(evaluate(&Cna::dev(), content(), &game, &response(&game, Side::Axis, plan)), Err(Rejection::Illegal { .. })));
        assert_eq!(bytes(&game), before);
    }
    for choice in [load(false), json!({"decision":"forfeit","guns":null,"ordnance":null})] {
        let (_, plan) = action(&game, Side::Axis, choice);
        request.space.check(&plan).unwrap();
        let accepted = respond(&game, Side::Axis, plan);
        assert_eq!(accepted.state.air.runtime.preparation, game.state.air.runtime.preparation);
    }
    let old = json!({"plane":fighter(&game, Side::Axis).0,"mode":0});
    let decoded: PlaneDeclaration = serde_json::from_value(old).unwrap();
    assert!(decoded.initial_choice.is_none());
    assert!(serde_json::to_value(decoded).unwrap().get("initial_choice").is_none());
    assert!(serde_json::from_value::<InitialChoiceSelection>(json!({"decision":"load","surprise":true})).is_err());
}

/// Cases: scen:59.32, airlog:39.12
/// Interpretations: interp:air-0018
#[test]
fn old_buffered_unselected_omission_decodes_then_stops_atomically_without_forfeiture() {
    let game = opened();
    let (_, ready) = answered(&game, Side::Axis, load(true));
    let mut saved = serde_json::to_value(&ready).unwrap();
    saved["state"]["air"]["runtime"]["tactical"]["answers"]["axis"][0]["planes"][0].as_object_mut().unwrap().remove("initial_choice");
    let old: Game<Cna> = serde_json::from_value(saved).unwrap();
    let before = bytes(&old);
    assert!(matches!(evaluate(&Cna::dev(), content(), &old, &Command::Advance), Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "scen:59.32"));
    assert_eq!(bytes(&old), before);
    // Actual finish dispatch also preserves a nonempty caller event prefix.
    let mut direct = old.clone();
    let mut rng = CampaignRng::from_state(&old.rng);
    let mut events = vec![cna_core::event::EngineEvent::public(cna_protocol::GameEvent::Note { text: "existing prefix".into() })];
    let prefix = events.clone();
    let error = Cna::dev().finish_step(content(), &mut direct.state, &mut cna_core::engine::Cx { rng: &mut rng, events: &mut events }).unwrap_err();
    assert!(matches!(error, EngineError::Unsupported { case, .. } if case == "scen:59.32"));
    assert_eq!(bytes(&direct), before);
    assert_eq!(rng.state(), old.rng);
    assert_eq!(events, prefix);
}

/// Cases: scen:59.32, airlog:34.8, airlog:34.84, airlog:39.12
/// Interpretations: interp:air-0018
#[test]
fn private_legacy_fixture_and_real_arrival_cannot_use_coarse_flags_or_gt1_as_credit() {
    let mut legacy = opened();
    let (id, plan) = action(&legacy, Side::Axis, load(true));
    // Negative checkpoint provenance fixture, never a positive fresh setup.
    legacy.state.air.runtime.preparation.initial_choices.remove(&id);
    legacy.state.air.runtime.preparation.current.insert(id.clone(), CurrentPreparation::UnknownLegacy);
    inventory::check(content(), &legacy.state.air).unwrap();
    let before = bytes(&legacy);
    assert!(matches!(evaluate(&Cna::dev(), content(), &legacy, &response(&legacy, Side::Axis, plan)), Err(Rejection::Illegal { .. })));
    assert_eq!(bytes(&legacy), before);
    let aircraft = legacy.state.air.runtime.aircraft[&id].aircraft.clone();
    let arrived = inventory::receive_unassigned(content(), &mut legacy.state.air, Side::Axis, &aircraft, 1).unwrap();
    let selected = PlaneDeclaration { plane: arrived[0].clone(), mode: 0, rated_pilot: None, initial_choice: Some(serde_json::from_value(load(true)).unwrap()) };
    assert!(!legacy.state.air.runtime.preparation.initial_choices.contains_key(&selected.plane));
    assert!(matches!(validate_answer(content(), &legacy.state, Side::Axis, &selected), Err(Rejection::Illegal { .. })));
    let old = legacy.state.air.runtime.preparation.clone();
    inventory::initialize(content(), &mut legacy.state).unwrap();
    assert_eq!(legacy.state.air.runtime.preparation, old);
}

/// Cases: scen:59.32, airlog:34.14, airlog:34.17, airlog:39.12
/// Interpretations: interp:air-0018
#[test]
fn both_owner_source_checks_precede_initial_commit_and_late_real_draft_failure_rolls_back() {
    let game = opened();
    let (axis, plan) = action(&game, Side::Axis, load(true));
    let one = respond(&game, Side::Axis, plan);
    let (cw, plan) = action(&one, Side::Commonwealth, load(true));
    let mut ready = respond(&one, Side::Commonwealth, plan);
    let before = bytes(&ready);
    let mut changed = content().clone();
    let plane = &ready.state.air.runtime.aircraft[&cw];
    let selected = &ready.state.air.runtime.tactical.answers[&Side::Commonwealth][0].planes[0];
    changed.units.aircraft.get_mut(&plane.aircraft).unwrap().modes[selected.mode].fuel_points = None;
    assert!(matches!(missions::finish(&changed, &mut ready.state), Err(EngineError::Unsupported { case, .. }) if case == "airlog:38.21"));
    assert_eq!(bytes(&ready), before);
    let mut observed = false;
    let error = missions::finish_transaction(content(), &mut ready.state, |draft| {
        assert_eq!(draft.air.runtime.preparation.initial_choices[&axis], InitialChoice::Committed);
        assert_eq!(draft.air.runtime.preparation.initial_choices[&cw], InitialChoice::Committed);
        assert!(matches!(draft.air.runtime.preparation.current[&axis], CurrentPreparation::KnownLoad { .. }));
        observed = true;
        let aircraft = draft.air.runtime.aircraft[&axis].aircraft.clone();
        draft.air.forces.get_mut("axis").unwrap().planes.get_mut(&aircraft).unwrap().total += 1;
    }).unwrap_err();
    assert!(observed && matches!(error, EngineError::Invariant { .. }));
    assert_eq!(bytes(&ready), before);
}

/// Cases: scen:59.32, airlog:34.0, airlog:39.12
/// Interpretations: interp:air-0018
#[test]
fn admission_binds_full_before_and_generic_callbacks_cannot_forge_or_resurrect_tombstones() {
    let game = opened();
    let (id, mut ready) = answered(&game, Side::Axis, load(true));
    let token = prepare(content(), &ready.state, missions::period(&ready.state).unwrap()).unwrap().unwrap();
    inventory::update(content(), &mut ready.state.air, |runtime| {
        let next = !runtime.aircraft[&id].refitted;
        runtime.aircraft.get_mut(&id).unwrap().refitted = next;
        Ok(())
    }).unwrap();
    let before = bytes(&ready);
    assert!(matches!(inventory::update_initial_loads(content(), &mut ready.state.air, token), Err(EngineError::Invariant { .. })));
    assert_eq!(bytes(&ready), before);
    let (_, ready) = answered(&game, Side::Axis, json!({"decision":"forfeit"}));
    let mut done = evaluate(&Cna::dev(), content(), &ready, &Command::Advance).unwrap().game;
    let before = bytes(&done);
    assert!(inventory::update(content(), &mut done.state.air, |runtime| {
        runtime.preparation.initial_choices.insert(id.clone(), InitialChoice::Unselected);
        runtime.preparation.current.insert(id.clone(), CurrentPreparation::InitialUnspecified);
        Ok(())
    }).is_err());
    assert_eq!(bytes(&done), before);
    let retired = done.state.air.runtime.aircraft[&id].clone();
    inventory::update(content(), &mut done.state.air, |runtime| {
        runtime.aircraft.remove(&id);
        runtime.preparation.current.remove(&id);
        Ok(())
    }).unwrap();
    assert_eq!(done.state.air.runtime.preparation.initial_choices[&id], InitialChoice::Forfeited);
    let before = bytes(&done);
    assert!(inventory::update(content(), &mut done.state.air, |runtime| {
        runtime.aircraft.insert(id.clone(), retired);
        runtime.preparation.current.insert(id, CurrentPreparation::Unprepared);
        Ok(())
    }).is_err());
    assert_eq!(bytes(&done), before);
}

/// Cases: scen:59.32, airlog:39.12, land:3.6
/// Interpretations: interp:air-0018
#[test]
fn enemy_initial_rights_and_actual_load_forfeit_choices_preserve_action_and_advance_streams() {
    let game = opened();
    let mut legacy = game.clone();
    for (id, plane) in &legacy.state.air.runtime.aircraft {
        if plane.force == "axis" {
            legacy.state.air.runtime.preparation.initial_choices.remove(id);
            legacy.state.air.runtime.preparation.current.insert(id.clone(), CurrentPreparation::UnknownLegacy);
        }
    }
    inventory::check(content(), &legacy.state.air).unwrap();
    assert_action_indistinguishable(&Cna::dev(), content(), &game, &legacy, &response(&game, Side::Commonwealth, Value::Null), Side::Commonwealth);
    for alternative in [load(false), json!({"decision":"forfeit"})] {
        let (_, left) = action(&game, Side::Axis, load(true));
        let (_, right) = action(&game, Side::Axis, alternative.clone());
        assert_actions_indistinguishable(&Cna::dev(), content(), (&game, &response(&game, Side::Axis, left)), (&game, &response(&game, Side::Axis, right)), Side::Commonwealth);
        let (_, left) = answered(&game, Side::Axis, load(true));
        let (_, right) = answered(&game, Side::Axis, alternative);
        assert_action_indistinguishable(&Cna::dev(), content(), &left, &right, &Command::Advance, Side::Commonwealth);
        let a = evaluate(&Cna::dev(), content(), &left, &Command::Advance).unwrap().game;
        let b = evaluate(&Cna::dev(), content(), &right, &Command::Advance).unwrap().game;
        assert_indistinguishable(&Cna::dev(), content(), &a.state, &b.state, Side::Commonwealth);
    }
    let (side, id, modes) = Side::ALL.into_iter().find_map(|side| {
        missions::own_planes(&game.state, side).into_iter().find_map(|id| {
            let source = &content().units.aircraft[&game.state.air.runtime.aircraft[&id].aircraft];
            let modes: Vec<_> = source.modes.iter().enumerate().filter(|(_, row)| {
                row.fuel_points.is_some() && ["b", "d"].into_iter().any(|letter| row.missions.get(letter).is_some_and(|code| matches!(code.as_str(), "day" | "night")))
            }).map(|(mode, _)| mode).collect();
            (modes.len() > 1).then_some((side, id, modes))
        })
    }).expect("actual initial bombing aircraft with two source modes");
    let mission = content().tables.airlog.mission_summary.missions().iter().find(|row| {
        row.class == Some(cna_tables::airlog::air::MissionClass::Bombing)
            && missions::supported(&row.mission)
            && content().tables.airlog.mission_summary.land_support(&row.mission).is_some()
    }).unwrap();
    let target = &content().map.iter().next().unwrap().id;
    let plans: Vec<_> = modes.iter().take(2).map(|mode| json!([{
        "mission":mission.mission,"light":"day","target":{"hex":target,"scramble_reserve":false},
        "planes":[{"plane":id,"mode":mode,"initial_choice":load(false)}]
    }])).collect();
    assert_actions_indistinguishable(&Cna::dev(), content(), (&game, &response(&game, side, plans[0].clone())), (&game, &response(&game, side, plans[1].clone())), side.opponent());
    let left = respond(&respond(&game, side, plans[0].clone()), side.opponent(), Value::Null);
    let right = respond(&respond(&game, side, plans[1].clone()), side.opponent(), Value::Null);
    assert_action_indistinguishable(&Cna::dev(), content(), &left, &right, &Command::Advance, side.opponent());
}

/// Cases: scen:59.32, airlog:34.14, airlog:38.44, airlog:39.12, airlog:41.39
/// Interpretations: interp:air-0018
#[test]
fn explicit_source_bombs_commit_on_real_declaration_and_mines_retain_exact_rating() {
    let game = opened();
    let (id, mode, points) = missions::own_planes(&game.state, Side::Axis).into_iter().find_map(|id| {
        let source = &content().units.aircraft[&game.state.air.runtime.aircraft[&id].aircraft];
        source.modes.iter().enumerate().find_map(|(mode, row)| {
            row.bomb_capacity.filter(|points| *points > 0).filter(|_| ["b", "d"].into_iter().any(|letter| row.missions.get(letter).is_some_and(|code| matches!(code.as_str(), "day" | "night"))))
                .map(|points| (id.clone(), mode, points))
        })
    }).expect("actual initial Axis aircraft with printed bomb capacity");
    let mission = content().tables.airlog.mission_summary.missions().iter().find(|row| {
        row.class == Some(cna_tables::airlog::air::MissionClass::Bombing)
            && missions::supported(&row.mission)
            && content().tables.airlog.mission_summary.land_support(&row.mission).is_some()
    }).unwrap();
    let target = &content().map.iter().next().unwrap().id;
    let plan = json!([{"mission":mission.mission,"light":"day","target":{"hex":target,"scramble_reserve":false},
        "planes":[{"plane":id,"mode":mode,"initial_choice":{"decision":"load","guns":false,"ordnance":{"kind":"bombs","points":points}}}]}]);
    let mut empty_plan = plan.clone();
    empty_plan[0]["planes"][0]["initial_choice"]["ordnance"] = json!({"kind":"none"});
    assert_actions_indistinguishable(&Cna::dev(), content(), (&game, &response(&game, Side::Axis, plan.clone())), (&game, &response(&game, Side::Axis, empty_plan.clone())), Side::Commonwealth);
    let ready = respond(&respond(&game, Side::Axis, plan), Side::Commonwealth, Value::Null);
    let empty_ready = respond(&respond(&game, Side::Axis, empty_plan), Side::Commonwealth, Value::Null);
    assert_action_indistinguishable(&Cna::dev(), content(), &ready, &empty_ready, &Command::Advance, Side::Commonwealth);
    let done = evaluate(&Cna::dev(), content(), &ready, &Command::Advance).unwrap().game;
    let CurrentPreparation::KnownLoad { load: recorded } = &done.state.air.runtime.preparation.current[&id] else { panic!() };
    assert_eq!(recorded.mode, mode);
    assert_eq!(recorded.ordnance, Some(CarriedOrdnance::Bombs { bomb_points: points }));
    assert!(recorded.fuel_available && !recorded.gun_ammunition);
    assert_eq!(serde_json::to_value(&done.state.logistics).unwrap(), serde_json::to_value(&game.state.logistics).unwrap());
    // Structural/source selection control, not a mining mission or mine count.
    let mut selected = PlaneDeclaration { plane: id, mode, rated_pilot: None, initial_choice: Some(InitialChoiceSelection {
        decision: InitialDecision::Load, guns: Some(false), ordnance: Some(InitialOrdnance { kind: InitialOrdnanceKind::Mines, points: Some(points) }),
    }) };
    validate_answer(content(), &game.state, Side::Axis, &selected).unwrap();
    selected.initial_choice.as_mut().unwrap().ordnance.as_mut().unwrap().points = Some(points.checked_add(1).unwrap());
    assert!(matches!(validate_answer(content(), &game.state, Side::Axis, &selected), Err(Rejection::Illegal { .. })));
}
