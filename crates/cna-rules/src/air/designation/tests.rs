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
use std::sync::OnceLock;
fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn add_squadron(state: &mut State, side: Side) {
    let force = if side == Side::Axis {
        "axis"
    } else {
        "commonwealth"
    };
    let reserve = state.air.forces.get_mut(force).unwrap();
    let aircraft = reserve
        .planes
        .iter()
        .find(|(_, c)| c.total > 0)
        .unwrap()
        .0
        .clone();
    let c = reserve.planes.get_mut(&aircraft).unwrap();
    c.total -= 1;
    c.ready -= 1;
    c.fuelled -= 1;
    c.armed -= 1;
    let id = format!("{force}.test");
    state.air.squadrons.insert(
        id.clone(),
        AirSquadron {
            id,
            force: force.into(),
            side,
            nationality: if side == Side::Axis { "it" } else { "cw" }.into(),
            facility: if side == Side::Axis {
                "airfield_benina"
            } else {
                "airfield_abbassia"
            }
            .into(),
            initial_aircraft: None,
            planes: BTreeMap::from([(
                aircraft,
                PlaneCount {
                    total: 1,
                    ready: 1,
                    fuelled: 1,
                    armed: 1,
                },
            )]),
            pilots: BTreeMap::new(),
        },
    );
}
fn fixture(axis: bool, cw: bool) -> State {
    let mut s = State::new(content()).unwrap();
    s.setup.closed = true;
    // This fixture tests Air disclosure; unrelated ground inspection is bounded.
    s.land.units.clear();
    s.logistics.unit_supply.clear();
    s.logistics.truck_pools.clear();
    s.logistics.dumps.clear();
    if axis {
        add_squadron(&mut s, Side::Axis);
    }
    if cw {
        add_squadron(&mut s, Side::Commonwealth);
    }
    s.cursor.block = crate::seq::Block::Pre;
    s.cursor.index = 1;
    s.cursor.entered = true;
    s
}
fn opened(axis: bool, cw: bool) -> Game<Cna> {
    let mut s = fixture(axis, cw);
    let mut rng = CampaignRng::from_seed([17; 32]);
    let before = rng.state();
    let mut events = Vec::new();
    enter(
        content(),
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert_eq!(rng.state(), before);
    Game {
        state: s,
        rng: rng.state(),
    }
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
fn respond(g: Game<Cna>, side: Side, action: Value) -> Game<Cna> {
    evaluate(&Cna::full(), content(), &g, &command(&g, side, action))
        .unwrap()
        .game
}

/// Cases: airlog:41.0, airlog:39.16, land:3.6
#[test]
fn dispatcher_enters_fixed_air_windows_without_revealing_enemy_eligibility() {
    let make = |axis| {
        let mut state = fixture(axis, true);
        state.cursor.entered = false;
        Game::<Cna> {
            state,
            rng: CampaignRng::from_seed([17; 32]).state(),
        }
    };
    let a = make(true);
    let b = make(false);
    assert_action_indistinguishable(
        &Cna::full(),
        content(),
        &a,
        &b,
        &Command::Advance,
        Side::Commonwealth,
    );
    for start in [a, b] {
        let result = evaluate(&Cna::full(), content(), &start, &Command::Advance).unwrap();
        assert_eq!(result.game.rng, start.rng);
        assert!(result.game.state.cursor.entered);
        assert_eq!(result.game.state.cursor.index, start.state.cursor.index);
        assert_eq!(result.game.state.decisions.pending.len(), 2);
        for pending in &result.game.state.decisions.pending {
            assert_eq!(pending.seat.role, Role::Air);
            assert_eq!(pending.kind, KIND);
            assert_eq!(pending.secrecy, Secrecy::SecretSimultaneous);
        }
        assert!(
            result
                .game
                .state
                .air
                .runtime
                .designation
                .assignments
                .is_empty()
        );
    }
}

/// Cases: airlog:41.0, airlog:39.16
#[test]
fn both_private_windows_exist_even_with_no_eligible_squadrons() {
    let g = opened(false, false);
    assert_eq!(g.state.decisions.pending.len(), 2);
    for p in &g.state.decisions.pending {
        assert_eq!(p.seat.role, Role::Air);
        assert_eq!(p.secrecy, Secrecy::SecretSimultaneous);
        assert!(p.space.check(&Value::Null).is_ok());
        assert!(p.space.check(&json!({})).is_ok());
        assert!(p.space.check(&json!({"foreign":"land_support"})).is_err());
    }
}

/// Cases: airlog:41.0, airlog:39.16
#[test]
fn answers_record_only_then_finish_commits_whole_squadrons_and_replays() {
    let start = opened(true, true);
    let inventory = start.state.air.runtime.aircraft.clone();
    let rng = start.rng.clone();
    let a = respond(start, Side::Axis, json!({"axis.test":"malta_raid"}));
    assert!(!a.state.air.runtime.designation.finished);
    assert!(a.state.air.runtime.designation.assignments.is_empty());
    let checkpoint = serde_json::to_value(&a).unwrap();
    let restored: Game<Cna> = serde_json::from_value(checkpoint).unwrap();
    let mut a = respond(a, Side::Commonwealth, json!({"commonwealth.test":"convoy"}));
    let mut b = respond(
        restored,
        Side::Commonwealth,
        json!({"commonwealth.test":"convoy"}),
    );
    assert!(a.state.air.runtime.designation.assignments.is_empty());
    finish(content(), &mut a.state).unwrap();
    finish(content(), &mut b.state).unwrap();
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&b).unwrap()
    );
    assert_eq!(a.rng, rng);
    assert_eq!(a.state.air.runtime.aircraft, inventory);
    assert_eq!(
        squadron_family(&a.state, "axis.test"),
        Some(Family::MaltaRaid)
    );
    assert_eq!(
        squadron_family(&a.state, "commonwealth.test"),
        Some(Family::Convoy)
    );
    assert_eq!(squadron_family(&a.state, "unarrived"), None);
    assert!(
        own_report(&a.state, Side::Axis)["overrides"]
            .get("commonwealth.test")
            .is_none()
    );
    assert!(a.state.air.runtime.designation.waiting.is_empty());
    let finished = serde_json::to_value(&a).unwrap();
    finish(content(), &mut a.state).unwrap();
    assert_eq!(finished, serde_json::to_value(&a).unwrap());
    super::super::inventory::check(content(), &a.state.air).unwrap();
}

/// Cases: airlog:41.0, airlog:39.16, land:3.6
#[test]
fn hidden_enemy_squadron_eligibility_has_fixed_schedule_and_acceptance() {
    let a = opened(true, true);
    let b = opened(false, true);
    assert_eq!(
        a.state.decisions.pending.len(),
        b.state.decisions.pending.len()
    );
    assert_indistinguishable(
        &Cna::full(),
        content(),
        &a.state,
        &b.state,
        Side::Commonwealth,
    );
    assert_action_indistinguishable(
        &Cna::full(),
        content(),
        &a,
        &b,
        &command(
            &a,
            Side::Commonwealth,
            json!({"commonwealth.test":"convoy"}),
        ),
        Side::Commonwealth,
    );
}

/// Cases: airlog:41.0, airlog:39.16, land:3.6
#[test]
fn hidden_enemy_pass_and_malta_declaration_have_identical_enemy_observations() {
    let g = opened(true, true);
    assert_actions_indistinguishable(
        &Cna::full(),
        content(),
        (&g, &command(&g, Side::Axis, Value::Null)),
        (
            &g,
            &command(&g, Side::Axis, json!({"axis.test":"malta_raid"})),
        ),
        Side::Commonwealth,
    );
    let mut a = respond(g.clone(), Side::Axis, Value::Null);
    let mut b = respond(g, Side::Axis, json!({"axis.test":"malta_raid"}));
    a = respond(a, Side::Commonwealth, Value::Null);
    b = respond(b, Side::Commonwealth, Value::Null);
    finish(content(), &mut a.state).unwrap();
    finish(content(), &mut b.state).unwrap();
    assert_indistinguishable(
        &Cna::full(),
        content(),
        &a.state,
        &b.state,
        Side::Commonwealth,
    );
    let targets = BTreeSet::from(["axis.test".into(), "commonwealth.test".into()]);
    assert_eq!(
        visible_to(
            &Cna::full(),
            content(),
            &a.state,
            Side::Commonwealth,
            &targets
        ),
        visible_to(
            &Cna::full(),
            content(),
            &b.state,
            Side::Commonwealth,
            &targets
        )
    );
}

/// Cases: airlog:41.0, airlog:39.16, land:3.6
#[test]
fn enemy_readiness_and_pilots_never_change_own_designation_schema_or_answer() {
    let a = opened(true, true);
    let mut b = a.clone();
    super::super::inventory::update(content(), &mut b.state.air, |rt| {
        for p in rt.aircraft.values_mut().filter(|p| p.force == "axis") {
            p.refitted = !p.refitted;
            p.fuelled = !p.fuelled;
            p.armed = !p.armed;
        }
        for p in rt.pilots.values_mut().filter(|p| p.force == "axis") {
            p.rating = 6;
        }
        Ok(())
    })
    .unwrap();
    assert_indistinguishable(
        &Cna::full(),
        content(),
        &a.state,
        &b.state,
        Side::Commonwealth,
    );
    assert_action_indistinguishable(
        &Cna::full(),
        content(),
        &a,
        &b,
        &command(&a, Side::Commonwealth, Value::Null),
        Side::Commonwealth,
    );
}

/// Cases: airlog:41.0, airlog:39.16
#[test]
fn malformed_foreign_side_family_and_missing_entries_fail_without_inventory_change() {
    let g = opened(true, true);
    for (side, action) in [
        (
            Side::Commonwealth,
            json!({"commonwealth.test":"malta_raid"}),
        ),
        (Side::Axis, json!({"commonwealth.test":"convoy"})),
        (Side::Axis, json!({})),
        (
            Side::Axis,
            json!({"axis.test":"convoy","other":"land_support"}),
        ),
        (Side::Axis, json!(["axis.test"])),
    ] {
        let before = serde_json::to_value(&g).unwrap();
        assert!(evaluate(&Cna::full(), content(), &g, &command(&g, side, action)).is_err());
        assert_eq!(before, serde_json::to_value(&g).unwrap());
    }
}

/// Cases: airlog:41.0, airlog:39.16
#[test]
fn next_turn_resets_only_planning_and_full_still_stops_at_next_missing_step() {
    let mut g = respond(
        respond(opened(true, true), Side::Axis, Value::Null),
        Side::Commonwealth,
        Value::Null,
    );
    finish(content(), &mut g.state).unwrap();
    assert!(evaluate(&Cna::full(), content(), &g, &Command::Advance).is_err());
    let planes = g.state.air.runtime.aircraft.clone();
    g.state.cursor.game_turn += 1;
    assert_eq!(squadron_family(&g.state, "axis.test"), None);
    let mut rng = CampaignRng::from_state(&g.rng);
    let mut events = Vec::new();
    enter(
        content(),
        &mut g.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert_eq!(g.state.decisions.pending.len(), 2);
    assert_eq!(g.state.air.runtime.aircraft, planes);
    assert!(g.state.air.runtime.designation.assignments.is_empty());
}

/// Cases: airlog:41.0, airlog:39.16, land:3.6
#[test]
fn own_air_and_commander_reports_include_only_own_families_and_enemy_inspect_is_denied() {
    let mut g = respond(
        respond(
            opened(true, true),
            Side::Axis,
            json!({"axis.test":"malta_raid"}),
        ),
        Side::Commonwealth,
        json!({"commonwealth.test":"convoy"}),
    );
    finish(content(), &mut g.state).unwrap();
    for role in [Role::Air, Role::Commander] {
        let p = cna_core::visibility::Perspective::Seat(SeatId::new(Side::Axis, role));
        let report = Cna::full().observe(content(), &g.state, p);
        assert_eq!(
            report["air"]["designation"]["axis"]["overrides"]["axis.test"],
            json!("malta_raid")
        );
        assert!(report["air"]["designation"].get("commonwealth").is_none());
        assert!(
            Cna::full()
                .inspect(content(), &g.state, p, "commonwealth.test")
                .is_err()
        );
        let own = Cna::full()
            .inspect(content(), &g.state, p, "axis.test")
            .unwrap();
        assert_eq!(own["designation"]["family"], json!("malta_raid"));
        assert_eq!(
            own["designation"]["game_turn"],
            json!(g.state.cursor.game_turn)
        );
    }
    let p = cna_core::visibility::Perspective::Seat(SeatId::new(Side::Commonwealth, Role::Air));
    assert!(
        Cna::full()
            .inspect(content(), &g.state, p, "axis.test")
            .is_err()
    );
}

/// Cases: airlog:41.0, airlog:39.16, land:3.6
#[test]
fn ground_observation_keeps_existing_own_squadron_count_guard() {
    let mut g = respond(
        respond(
            opened(true, true),
            Side::Axis,
            json!({"axis.test":"convoy"}),
        ),
        Side::Commonwealth,
        Value::Null,
    );
    finish(content(), &mut g.state).unwrap();
    for role in [Role::FrontLine, Role::RearArea, Role::Logistics] {
        let p = cna_core::visibility::Perspective::Seat(SeatId::new(Side::Axis, role));
        let report = Cna::full().observe(content(), &g.state, p);
        assert!(
            report["air"]["squadrons"]
                .as_str()
                .unwrap()
                .starts_with("1 own squadrons")
        );
        assert!(report["air"]["designation"].is_null());
        assert!(!report["air"].to_string().contains("axis.test"));
        // Existing explicit own-squadron inspection permissions do not change.
        let own = Cna::full()
            .inspect(content(), &g.state, p, "axis.test")
            .unwrap();
        assert_eq!(own["designation"]["family"], json!("convoy"));
    }
}

/// Cases: airlog:41.0, airlog:39.16, land:3.6
#[test]
fn compact_report_reconstructs_only_current_own_committed_families() {
    for (axis, cw) in [(false, false), (true, true)] {
        for axis_choice in [
            Value::Null,
            json!({"axis.test":"convoy"}),
            json!({"axis.test":"malta_raid"}),
        ] {
            if !axis && !axis_choice.is_null() {
                continue;
            }
            let mut g = respond(
                respond(opened(axis, cw), Side::Axis, axis_choice),
                Side::Commonwealth,
                Value::Null,
            );
            finish(content(), &mut g.state).unwrap();
            // This view fixture adds later roster identities without asserting
            // arrival/SGSU entitlement. Neither has a committed designation.
            if cw {
                for (id, force) in [
                    ("commonwealth.late", "commonwealth"),
                    ("malta.group", "malta"),
                ] {
                    let mut squadron = g.state.air.squadrons["commonwealth.test"].clone();
                    squadron.id = id.into();
                    squadron.force = force.into();
                    squadron.planes.clear();
                    squadron.pilots.clear();
                    g.state.air.squadrons.insert(id.into(), squadron);
                }
            }
            for side in SIDES {
                let report = own_report(&g.state, side);
                assert_eq!(report["default_family"], json!("land_support"));
                let overrides = report["overrides"].as_object().unwrap();
                assert!(
                    overrides
                        .keys()
                        .all(|id| g.state.air.squadrons[id].side == side)
                );
                let mut reconstructed = BTreeMap::new();
                for squadron in g.state.air.squadrons.values().filter(|s| s.side == side) {
                    let family: Option<Family> = match overrides.get(&squadron.id) {
                        Some(value) => serde_json::from_value(value.clone()).unwrap(),
                        None => Some(Family::LandSupport),
                    };
                    assert_eq!(family, squadron_family(&g.state, &squadron.id));
                    if let Some(family) = family {
                        reconstructed.insert(squadron.id.clone(), family);
                    }
                }
                let expected: BTreeMap<_, _> = g
                    .state
                    .air
                    .runtime
                    .designation
                    .assignments
                    .iter()
                    .filter(|(id, _)| {
                        g.state
                            .air
                            .squadrons
                            .get(*id)
                            .is_some_and(|s| s.side == side)
                    })
                    .map(|(id, family)| (id.clone(), *family))
                    .collect();
                assert_eq!(reconstructed, expected);
                if side == Side::Commonwealth && cw {
                    assert_eq!(overrides.get("commonwealth.late"), Some(&Value::Null));
                    assert_eq!(overrides.get("malta.group"), Some(&Value::Null));
                }
            }
        }
    }
}

/// Cases: airlog:41.0, airlog:39.16, land:3.6
#[test]
fn compact_report_has_no_default_before_finish_or_in_a_different_turn() {
    let g = opened(true, true);
    for side in SIDES {
        assert_eq!(
            own_report(&g.state, side),
            json!({"game_turn":g.state.cursor.game_turn,"assignments":{}})
        );
    }
    let mut g = respond(
        respond(g, Side::Axis, Value::Null),
        Side::Commonwealth,
        Value::Null,
    );
    assert!(
        own_report(&g.state, Side::Axis)
            .get("default_family")
            .is_none()
    );
    finish(content(), &mut g.state).unwrap();
    assert_eq!(own_report(&g.state, Side::Axis)["overrides"], json!({}));
    g.state.cursor.game_turn += 1;
    for side in SIDES {
        assert_eq!(
            own_report(&g.state, side),
            json!({"game_turn":g.state.cursor.game_turn,"assignments":{}})
        );
        assert!(own_report(&g.state, side).get("default_family").is_none());
    }
}
