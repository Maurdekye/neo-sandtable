use super::*;
use crate::{
    Cna,
    state::{AirSquadron, PlaneCount},
    testkit::{
        assert_action_indistinguishable, assert_actions_indistinguishable,
        assert_indistinguishable, visible_to,
    },
};
use cna_core::{
    decision::DecisionResponse,
    dice::CampaignRng,
    engine::{Command, Game, Ruleset, evaluate},
};
use serde_json::json;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn squadron(state: &mut State, side: Side) {
    let (force, aircraft, facility, nationality) = match side {
        Side::Axis => ("axis", "it.cr42", "airfield_benina", "it"),
        Side::Commonwealth => ("commonwealth", "cw.hurricane_i", "airfield_abbassia", "cw"),
    };
    let count = state
        .air
        .forces
        .get_mut(force)
        .unwrap()
        .planes
        .get_mut(aircraft)
        .unwrap();
    count.total -= 2;
    count.ready -= 2;
    count.fuelled -= 2;
    count.armed -= 2;
    let id = format!("{force}.test");
    state.air.squadrons.insert(
        id.clone(),
        AirSquadron {
            id,
            force: force.into(),
            side,
            nationality: nationality.into(),
            facility: facility.into(),
            initial_aircraft: None,
            planes: BTreeMap::from([(
                aircraft.into(),
                PlaneCount {
                    total: 2,
                    ready: 2,
                    fuelled: 2,
                    armed: 2,
                },
            )]),
            pilots: BTreeMap::new(),
        },
    );
}
fn fixture(axis: bool, cw: bool) -> Game<Cna> {
    let mut state = State::new(content()).unwrap();
    state.setup.closed = true;
    state.land.units.clear();
    state.logistics.unit_supply.clear();
    state.logistics.truck_pools.clear();
    state.logistics.dumps.clear();
    if axis {
        squadron(&mut state, Side::Axis);
    }
    if cw {
        squadron(&mut state, Side::Commonwealth);
    }
    inventory::initialize(content(), &mut state).unwrap();
    state.air.runtime.designation.game_turn = Some(state.cursor.game_turn);
    state.air.runtime.designation.finished = true;
    state.air.runtime.designation.assignments = state
        .air
        .squadrons
        .values()
        .filter(|s| s.force != "malta")
        .map(|s| (s.id.clone(), Family::LandSupport))
        .collect();
    state.cursor.block = crate::seq::Block::OpStage;
    state.cursor.op_stage = Some(1);
    state.cursor.index = crate::seq::OPSTAGE
        .iter()
        .position(|s| s.anchor == ANCHOR)
        .unwrap();
    state.cursor.entered = false;
    Game {
        state,
        rng: CampaignRng::from_seed([39; 32]).state(),
    }
}
fn opened(axis: bool, cw: bool) -> Game<Cna> {
    evaluate(
        &Cna::dev(),
        content(),
        &fixture(axis, cw),
        &Command::Advance,
    )
    .unwrap()
    .game
}
fn command(g: &Game<Cna>, side: Side, action: Value) -> Command {
    let p = g
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == side)
        .unwrap();
    Command::Respond(DecisionResponse {
        decision_id: p.id.clone(),
        seat: p.seat,
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: p.id.to_string(),
        action,
        public_explanation: None,
    })
}
fn respond(g: &Game<Cna>, side: Side, action: Value) -> Game<Cna> {
    evaluate(&Cna::dev(), content(), g, &command(g, side, action))
        .unwrap()
        .game
}
fn plan(g: &Game<Cna>, side: Side) -> Value {
    let id = own_planes(&g.state, side)[0].clone();
    json!([{"mission":"scramble","light":"day","target":{"scramble_reserve":true},
        "planes":[{"plane":id,"mode":0}]}])
}
fn air_except_tactical(g: &Game<Cna>) -> Value {
    let mut value = serde_json::to_value(&g.state.air).unwrap();
    value["runtime"].as_object_mut().unwrap().remove("tactical");
    value
}

/// Cases: airlog:39.12, land:3.6
#[test]
fn full_uniform_entry_refusal_precedes_even_invalid_inventory() {
    let a = fixture(true, true);
    let mut b = fixture(false, false);
    b.state.air.runtime.initialized = false;
    b.state.air.runtime.bases_initialized = false;
    let first = evaluate(&Cna::full(), content(), &a, &Command::Advance).unwrap_err();
    let second = evaluate(&Cna::full(), content(), &b, &Command::Advance).unwrap_err();
    assert!(
        matches!(&first, Rejection::Engine(EngineError::Unsupported { case, detail })
        if case == "airlog:39.12" && detail.contains("transport") && detail.contains("dual"))
    );
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    assert_eq!(a.state.air.runtime.tactical, TacticalState::default());
}

/// Cases: airlog:39.12, airlog:39.16, land:3.6
#[test]
fn dev_fixed_pass_windows_and_private_limits_do_not_reveal_enemy_eligibility() {
    let a = fixture(true, true);
    let b = fixture(false, true);
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &a,
        &b,
        &Command::Advance,
        Side::Commonwealth,
    );
    for g in [fixture(false, false), a] {
        let result = evaluate(&Cna::dev(), content(), &g, &Command::Advance).unwrap();
        assert_eq!(result.game.rng, g.rng);
        assert_eq!(air_except_tactical(&result.game), air_except_tactical(&g));
        assert_eq!(result.game.state.decisions.pending.len(), 2);
        for p in &result.game.state.decisions.pending {
            assert_eq!(p.kind, KIND);
            assert_eq!(p.seat.role, Role::Air);
            assert_eq!(p.secrecy, Secrecy::SecretSimultaneous);
            let domain = Cna::dev()
                .pending(content(), &result.game.state)
                .into_iter()
                .find(|d| d.id == p.id)
                .unwrap();
            domain.space.check(&Value::Null).unwrap();
        }
        let limits: Vec<_> = result.events.iter().filter(|e| matches!(&e.event, GameEvent::Note { text } if text.contains("Development tactical"))).collect();
        assert_eq!(limits.len(), 2);
        for e in limits {
            assert!(matches!(e.audience, Audience::Side(_)));
        }
    }
}

/// Cases: airlog:39.12, airlog:39.16
#[test]
fn respond_buffers_only_finish_is_once_and_checkpoint_exact() {
    let start = opened(true, true);
    let air = air_except_tactical(&start);
    let rng = start.rng.clone();
    let a = respond(&start, Side::Axis, plan(&start, Side::Axis));
    assert_eq!(air_except_tactical(&a), air);
    assert_eq!(a.rng, rng);
    assert_eq!(a.state.air.runtime.tactical.answers.len(), 1);
    assert!(a.state.air.runtime.tactical.declarations.is_empty());
    let restored: Game<Cna> = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
    let a = respond(&a, Side::Commonwealth, Value::Null);
    let b = respond(&restored, Side::Commonwealth, Value::Null);
    assert_eq!(air_except_tactical(&a), air);
    assert_eq!(a.rng, rng);
    let mut a = a;
    let mut b = b;
    finish(content(), &mut a.state).unwrap();
    finish(content(), &mut b.state).unwrap();
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&b).unwrap()
    );
    assert_eq!(air_except_tactical(&a), air);
    assert_eq!(a.rng, rng);
    assert!(a.state.air.runtime.tactical.resolved);
    let id = MissionId(format!("axis-gt{}-op1-0", a.state.cursor.game_turn));
    assert_eq!(
        a.state.air.runtime.tactical.declarations[&id].target,
        DeclarationTarget::ScrambleReserve
    );
    let before = serde_json::to_value(&a).unwrap();
    finish(content(), &mut a.state).unwrap();
    assert_eq!(before, serde_json::to_value(&a).unwrap());
    inventory::check(content(), &a.state.air).unwrap();
}

/// Cases: airlog:39.12, airlog:39.16, land:3.6
#[test]
fn enemy_readiness_capacity_pilots_and_private_plans_have_no_oracle() {
    let a = opened(true, true);
    let mut b = a.clone();
    inventory::update(content(), &mut b.state.air, |rt| {
        for p in rt.aircraft.values_mut().filter(|p| p.force == "axis") {
            p.refitted = !p.refitted;
            p.fuelled = !p.fuelled;
            p.armed = !p.armed;
        }
        for p in rt.pilots.values_mut().filter(|p| p.force == "axis") {
            p.rating = 6;
        }
        let site = rt
            .facilities
            .get_mut(&super::super::facilities::FacilityId(
                "airfield_benina".into(),
            ))
            .unwrap();
        site.project_unavailable = true;
        Ok(())
    })
    .unwrap();
    let own = plan(&a, Side::Commonwealth);
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &a,
        &b,
        &command(&a, Side::Commonwealth, own),
        Side::Commonwealth,
    );
    assert_actions_indistinguishable(
        &Cna::dev(),
        content(),
        (&a, &command(&a, Side::Axis, Value::Null)),
        (&a, &command(&a, Side::Axis, plan(&a, Side::Axis))),
        Side::Commonwealth,
    );
    let mut empty = respond(
        &respond(&a, Side::Axis, Value::Null),
        Side::Commonwealth,
        Value::Null,
    );
    let mut declared = respond(
        &respond(&a, Side::Axis, plan(&a, Side::Axis)),
        Side::Commonwealth,
        Value::Null,
    );
    finish(content(), &mut empty.state).unwrap();
    finish(content(), &mut declared.state).unwrap();
    assert_indistinguishable(
        &Cna::dev(),
        content(),
        &empty.state,
        &declared.state,
        Side::Commonwealth,
    );
    let targets = a
        .state
        .air
        .runtime
        .aircraft
        .keys()
        .map(|id| format!("aircraft:{}", id.0))
        .collect();
    assert_eq!(
        visible_to(
            &Cna::dev(),
            content(),
            &empty.state,
            Side::Commonwealth,
            &targets
        ),
        visible_to(
            &Cna::dev(),
            content(),
            &declared.state,
            Side::Commonwealth,
            &targets
        )
    );
}

/// Cases: airlog:39.12, airlog:39.16
#[test]
fn actual_schema_rejects_foreign_duplicate_extra_and_wrong_target_atomically() {
    let g = opened(true, true);
    let good = plan(&g, Side::Axis);
    let mut foreign = good.clone();
    foreign[0]["planes"][0]["plane"] = json!(own_planes(&g.state, Side::Commonwealth)[0]);
    let mut extra = good.clone();
    extra[0]["enemy_unit"] = json!("hidden");
    let mut target = good.clone();
    target[0]["target"]["hex"] = json!("C4807");
    let mut wrong_mode = good.clone();
    wrong_mode[0]["planes"][0]["mode"] = json!(999);
    let duplicate = json!([good[0].clone(), good[0].clone()]);
    let before = serde_json::to_value(&g).unwrap();
    for bad in [foreign, extra, target, wrong_mode, duplicate] {
        assert!(evaluate(&Cna::dev(), content(), &g, &command(&g, Side::Axis, bad)).is_err());
        assert_eq!(before, serde_json::to_value(&g).unwrap());
    }
}

/// Cases: airlog:39.12, airlog:39.16
#[test]
fn late_source_failure_and_mirror_failure_leave_entire_finish_unchanged() {
    let start = opened(true, true);
    let g = respond(
        &respond(&start, Side::Axis, plan(&start, Side::Axis)),
        Side::Commonwealth,
        Value::Null,
    );
    let mut broken_content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    broken_content
        .units
        .aircraft
        .get_mut("it.cr42")
        .unwrap()
        .modes[0]
        .missions
        .insert("s".into(), "unreadable".into());
    let mut broken = g.clone();
    let before = serde_json::to_value(&broken).unwrap();
    assert!(
        matches!(finish(&broken_content, &mut broken.state), Err(EngineError::Unsupported { case, .. }) if case == "airlog:34.6")
    );
    assert_eq!(before, serde_json::to_value(&broken).unwrap());
    let mut broken = g;
    broken
        .state
        .air
        .squadrons
        .get_mut("axis.test")
        .unwrap()
        .planes
        .get_mut("it.cr42")
        .unwrap()
        .total += 1;
    let before = serde_json::to_value(&broken).unwrap();
    assert!(matches!(
        finish(content(), &mut broken.state),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(before, serde_json::to_value(&broken).unwrap());
}

/// Cases: airlog:39.12, airlog:39.16
#[test]
fn new_period_resets_only_intentions_and_ids_are_side_local() {
    let start = opened(true, true);
    let mut g = respond(
        &respond(&start, Side::Axis, plan(&start, Side::Axis)),
        Side::Commonwealth,
        plan(&start, Side::Commonwealth),
    );
    finish(content(), &mut g.state).unwrap();
    let old: BTreeSet<_> = g
        .state
        .air
        .runtime
        .tactical
        .declarations
        .keys()
        .cloned()
        .collect();
    let air = air_except_tactical(&g);
    g.state.cursor.op_stage = Some(2);
    let mut rng = CampaignRng::from_state(&g.rng);
    let mut events = Vec::new();
    enter(
        content(),
        &mut g.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert_eq!(air_except_tactical(&g), air);
    assert_eq!(g.state.air.runtime.tactical.answers.len(), 0);
    assert!(!g.state.air.runtime.tactical.resolved);
    let mut g = respond(
        &respond(&g, Side::Axis, plan(&g, Side::Axis)),
        Side::Commonwealth,
        Value::Null,
    );
    finish(content(), &mut g.state).unwrap();
    assert!(
        g.state
            .air
            .runtime
            .tactical
            .declarations
            .keys()
            .all(|id| !old.contains(id))
    );
    assert!(
        g.state
            .air
            .runtime
            .tactical
            .declarations
            .keys()
            .all(|id| id.0.contains("-op2-"))
    );
}

/// Cases: airlog:39.12, airlog:39.16
#[test]
fn absent_field_defaults_and_new_post_designation_planes_get_no_family() {
    let mut g = fixture(true, true);
    let new = inventory::receive_unassigned(content(), &mut g.state.air, Side::Axis, "it.cr42", 1)
        .unwrap();
    assert!(!own_planes(&g.state, Side::Axis).contains(&new[0]));
    let mut value = serde_json::to_value(&g).unwrap();
    value["state"]["air"]["runtime"]
        .as_object_mut()
        .unwrap()
        .remove("tactical");
    let restored: Game<Cna> = serde_json::from_value(value).unwrap();
    assert_eq!(
        restored.state.air.runtime.tactical,
        TacticalState::default()
    );
    assert_eq!(
        restored.state.air.runtime.aircraft,
        g.state.air.runtime.aircraft
    );
}

/// Cases: airlog:39.12, airlog:39.16, land:3.6
#[test]
fn actual_advance_closes_private_plans_once_with_equal_streams_and_recovery() {
    let start = opened(true, true);
    let a = respond(
        &respond(&start, Side::Axis, Value::Null),
        Side::Commonwealth,
        Value::Null,
    );
    let b = respond(
        &respond(&start, Side::Axis, plan(&start, Side::Axis)),
        Side::Commonwealth,
        Value::Null,
    );
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &a,
        &b,
        &Command::Advance,
        Side::Commonwealth,
    );
    let checkpoint: Game<Cna> = serde_json::from_value(serde_json::to_value(&b).unwrap()).unwrap();
    let first = evaluate(&Cna::dev(), content(), &b, &Command::Advance).unwrap();
    let replay = evaluate(&Cna::dev(), content(), &checkpoint, &Command::Advance).unwrap();
    assert_eq!(
        serde_json::to_value(&first.game).unwrap(),
        serde_json::to_value(&replay.game).unwrap()
    );
    assert_eq!(first.events, replay.events);
    assert_eq!(first.game.rng, start.rng);
    assert_eq!(
        air_except_tactical(&first.game),
        air_except_tactical(&start)
    );
    assert_eq!(first.game.state.air.runtime.tactical.declarations.len(), 1);
    assert!(first.game.state.air.runtime.tactical.resolved);
}
