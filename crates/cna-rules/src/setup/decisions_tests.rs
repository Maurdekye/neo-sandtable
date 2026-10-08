#![cfg(test)]
use super::*;
use crate::Cna;
use cna_core::decision::DecisionResponse;
use cna_core::dice::CampaignRng;
use cna_core::engine::{Command, Game, Ruleset, evaluate};
use cna_core::visibility::Perspective;
use serde_json::json;

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}
fn opened(c: &CnaContent) -> Game<Cna> {
    let game = Game {
        state: State::new(c).unwrap(),
        rng: CampaignRng::from_seed([5; 32]).state(),
    };
    evaluate(&Cna::dev(), c, &game, &Command::Advance)
        .unwrap()
        .game
}
fn submit(
    c: &CnaContent,
    game: &Game<Cna>,
    p: &Pending,
    action: Value,
) -> Result<Game<Cna>, Rejection> {
    evaluate(
        &Cna::dev(),
        c,
        game,
        &Command::Respond(DecisionResponse {
            decision_id: p.id.clone(),
            seat: p.seat,
            controller_epoch: 1,
            decision_revision: p.revision,
            idempotency_key: p.id.to_string(),
            action,
            public_explanation: None,
        }),
    )
    .map(|t| t.game)
}
fn first(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => json!(options[0].id),
        ActionSchema::Unit { among } => json!(among[0]),
        ActionSchema::Integer { max, .. } => json!(max),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .map(|f| (f.name.clone(), first(&f.schema)))
                .collect(),
        ),
        _ => panic!("unexpected setup schema"),
    }
}
fn finish(c: &CnaContent, mut game: Game<Cna>) -> Game<Cna> {
    for _ in 0..500 {
        if game.state.setup.closed {
            return game;
        }
        if game.state.decisions.pending.is_empty() {
            game = evaluate(&Cna::dev(), c, &game, &Command::Advance)
                .unwrap()
                .game;
            continue;
        }
        let p = game.state.decisions.pending[0].clone();
        game = submit(c, &game, &p, first(&p.space.schema)).unwrap();
    }
    panic!("setup failed to finish");
}

/// Cases: scen:59.2, scen:59.42, land:3.62
/// Interpretations: interp:scen-0005
#[test]
fn private_choices_publish_faces_only_after_the_entire_window_and_preserve_trucks() {
    let c = content();
    let initial = State::new(&c).unwrap();
    assert!(initial.land.movement.on_road.is_empty());
    let expected =
        initial
            .land
            .undistributed_trucks
            .values()
            .fold(Trucks::default(), |mut sum, t| {
                sum.light += t.light;
                sum.medium += t.medium;
                sum.heavy += t.heavy;
                sum
            });
    let mut game = opened(&c);
    assert!(
        game.state
            .decisions
            .pending
            .iter()
            .all(|p| p.secrecy == Secrecy::SecretSimultaneous)
    );
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == KIND_UNIT && p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    assert_eq!(p.seat.role, Role::Commander);
    let SetupTask::Unit { unit, .. } = &game.state.setup.tasks[&p.id] else {
        panic!();
    };
    let unit = unit.clone();
    // Regression input only: placement must discard stale ON membership at closure.
    game.state.land.movement.on_road.insert(unit.clone());
    let action = first(&p.space.schema);
    let enemy_before = Cna::dev().view(&c, &game.state, Perspective::Side(Side::Commonwealth));
    let game = submit(&c, &game, &p, action).unwrap();
    assert!(!game.state.setup.closed);
    assert!(game.state.land.movement.on_road.contains(&unit));
    assert!(matches!(
        game.state.land.units[&unit].location,
        Location::AwaitingSetup { .. }
    ));
    assert_eq!(
        Cna::dev().view(&c, &game.state, Perspective::Side(Side::Commonwealth)),
        enemy_before
    );
    let own = Cna::dev()
        .inspect(
            &c,
            &game.state,
            Perspective::Side(Side::Axis),
            unit.as_str(),
        )
        .unwrap();
    assert!(!own["setup_destination"].is_null());
    assert!(
        Cna::dev()
            .inspect(
                &c,
                &game.state,
                Perspective::Side(Side::Commonwealth),
                unit.as_str()
            )
            .is_err()
    );
    // A checkpoint retains the private choices and resumes without prematurely publishing them.
    let saved = serde_json::to_string(&game).unwrap();
    let restored: Game<Cna> = serde_json::from_str(&saved).unwrap();
    let game = finish(&c, game);
    let resumed = finish(&c, restored);
    assert!(!game.state.land.movement.on_road.contains(&unit));
    assert!(!resumed.state.land.movement.on_road.contains(&unit));
    assert_eq!(
        serde_json::to_value(&game).unwrap(),
        serde_json::to_value(&resumed).unwrap()
    );
    assert!(
        game.state.setup.unit_locations.is_empty() && game.state.setup.dump_locations.is_empty()
    );
    assert!(game.state.land.undistributed_trucks.is_empty());
    let actual = game
        .state
        .land
        .units
        .values()
        .fold(Trucks::default(), |mut sum, u| {
            sum.light += u.trucks.light;
            sum.medium += u.trucks.medium;
            sum.heavy += u.trucks.heavy;
            sum
        });
    assert_eq!(actual, expected);
    for u in game.state.land.units.values() {
        if matches!(u.location, Location::AwaitingSetup { .. }) {
            let (p, case) = group_placement(&c, &game.state, &u.id).unwrap();
            assert!(matches!(
                placement::choices(&c, &p, u.side, &case),
                Err(EngineError::Unsupported { .. })
            ));
        }
    }
    for d in game.state.logistics.dumps.values() {
        if let DumpLocation::AwaitingSetup { placement: p } = &d.location {
            assert!(matches!(
                placement::choices(&c, p, d.side, "scen:59.51"),
                Err(EngineError::Unsupported { .. })
            ));
        }
    }
    let enemy = Cna::dev().view(&c, &game.state, Perspective::Side(Side::Commonwealth));
    let hex = game.state.land.units[&unit].location.hex().unwrap();
    let stack = enemy
        .stacks
        .iter()
        .find(|s| s.side == Side::Axis && s.hex == hex.as_str())
        .unwrap();
    // Rules as written, the placed counters show the other side their faces only.
    assert_eq!(stack.visible_count, Some(stack.unit_ids.len() as u32));
    for id in &stack.unit_ids {
        crate::testkit::assert_face(&serde_json::to_value(&enemy.units[id]).unwrap());
    }
    if let Some(face) = enemy.units.get(unit.as_str()) {
        crate::testkit::assert_face(&serde_json::to_value(face).unwrap());
    }
}

/// Cases: scen:59.2, land:8.13
/// Interpretations: interp:scen-0005
#[test]
fn opposing_hidden_buffers_do_not_change_answer_validation_and_collision_retries_at_closure() {
    let mut c = content();
    let mut state = State::new(&c).unwrap();
    let selected: Vec<_> = c
        .scenario
        .land
        .iter_mut()
        .map(|f| {
            let g = &mut f.groups[0];
            g.placement = Placement::HexesAny {
                hexes: vec!["A0101".into(), "A0102".into()],
            };
            state
                .land
                .units
                .values()
                .find(|u| u.setup_group.as_ref() == Some(&g.id))
                .unwrap()
                .id
                .clone()
        })
        .collect();
    for u in state.land.units.values_mut() {
        u.location = Location::NotArrived;
    }
    state.land.undistributed_trucks.clear();
    state.logistics.dumps.clear();
    state.logistics.truck_pools.clear();
    state.air.forces.clear();
    for id in &selected {
        let u = state.land.units.get_mut(id).unwrap();
        u.location = Location::AwaitingSetup {
            group: u.setup_group.clone().unwrap(),
        };
    }
    let game = Game {
        state,
        rng: CampaignRng::from_seed([5; 32]).state(),
    };
    let game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
        .unwrap()
        .game;
    let first = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let second = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Commonwealth)
        .unwrap()
        .clone();
    let game = submit(&c, &game, &first, json!("A0101")).unwrap();
    let axis = selected
        .iter()
        .find(|id| game.state.land.units[*id].side == Side::Axis)
        .unwrap();
    let cw = selected
        .iter()
        .find(|id| game.state.land.units[*id].side == Side::Commonwealth)
        .unwrap();
    let mut alternate = game.clone();
    alternate.state.setup.unit_locations.insert(
        axis.clone(),
        Location::Hex {
            hex: "A0102".into(),
        },
    );
    crate::testkit::assert_indistinguishable(
        &Cna::dev(),
        &c,
        &game.state,
        &alternate.state,
        Side::Commonwealth,
    );
    let cmd = Command::Respond(DecisionResponse {
        decision_id: second.id.clone(),
        seat: second.seat,
        controller_epoch: 1,
        decision_revision: second.revision,
        idempotency_key: second.id.to_string(),
        action: json!("A0101"),
        public_explanation: None,
    });
    let accepted = evaluate(&Cna::dev(), &c, &game, &cmd).unwrap();
    let other = evaluate(&Cna::dev(), &c, &alternate, &cmd).unwrap();
    assert_eq!(
        serde_json::to_value(&accepted.events).unwrap(),
        serde_json::to_value(&other.events).unwrap()
    );
    assert_eq!(
        Cna::dev().view(
            &c,
            &accepted.game.state,
            Perspective::Side(Side::Commonwealth)
        ),
        Cna::dev().view(&c, &other.game.state, Perspective::Side(Side::Commonwealth))
    );
    assert!(!accepted.game.state.setup.closed);
    assert!(accepted.game.state.decisions.pending.is_empty());
    let retry_transition = evaluate(&Cna::dev(), &c, &accepted.game, &Command::Advance).unwrap();
    assert!(retry_transition.events.iter().any(|e| Perspective::Side(Side::Axis).can_see(&e.audience)
        && matches!(&e.event, GameEvent::UnitUpdated {unit} if unit.id == axis.as_str() && unit.hex.as_deref() == Some("A0101"))));
    assert!(
        !retry_transition
            .events
            .iter()
            .any(|e| matches!(&e.event, GameEvent::UnitUpdated {unit}
        if unit.id == cw.as_str() && unit.hex.as_deref() == Some("A0101")))
    );
    let retry = retry_transition.game;
    assert!(!retry.state.setup.closed);
    assert_eq!(
        retry.state.land.units[axis]
            .location
            .hex()
            .unwrap()
            .as_str(),
        "A0101"
    );
    assert!(matches!(
        retry.state.land.units[cw].location,
        Location::AwaitingSetup { .. }
    ));
    let p = retry
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == KIND_UNIT)
        .unwrap()
        .clone();
    assert_eq!(p.seat.side, Side::Commonwealth);
    assert!(submit(&c, &retry, &p, json!("A0101")).is_err());
    let accepted = submit(&c, &retry, &p, json!("A0102")).unwrap();
    assert!(!accepted.state.setup.closed);
    let completed = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
    assert!(completed.game.state.setup.closed);
    assert!(completed.events.iter().any(|e| Perspective::Side(Side::Commonwealth).can_see(&e.audience)
        && matches!(&e.event, GameEvent::UnitUpdated {unit} if unit.id == cw.as_str() && unit.hex.as_deref() == Some("A0102"))));
}

/// Cases: scen:59.42
#[test]
fn split_allocations_stay_with_the_group_and_reject_overdraw_and_foreign_units() {
    let c = content();
    let mut game = opened(&c);
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| {
            let Some(SetupTask::Trucks { group }) = game.state.setup.tasks.get(&p.id) else {
                return false;
            };
            game.state.land.undistributed_trucks[group].light > 1
                && game
                    .state
                    .land
                    .units
                    .values()
                    .filter(|u| u.setup_group.as_ref() == Some(group))
                    .count()
                    > 1
        })
        .unwrap()
        .clone();
    assert_eq!(p.seat.role, Role::Logistics);
    let SetupTask::Trucks { group } = &game.state.setup.tasks[&p.id] else {
        panic!();
    };
    let group = group.clone();
    let members: Vec<_> = game
        .state
        .land
        .units
        .values()
        .filter(|u| u.setup_group.as_ref() == Some(&group))
        .map(|u| u.id.clone())
        .collect();
    let original = game.state.land.undistributed_trucks[&group];
    let action = |unit: &UnitId, light, medium, heavy| json!({"unit":unit,"light":light,"medium":medium,"heavy":heavy});
    let before = serde_json::to_string(&game).unwrap();
    assert!(submit(&c, &game, &p, action(&members[0], original.light + 1, 0, 0)).is_err());
    let foreign = game
        .state
        .units_of(p.seat.side.opponent())
        .next()
        .unwrap()
        .id
        .clone();
    assert!(submit(&c, &game, &p, action(&foreign, 1, 0, 0)).is_err());
    assert_eq!(serde_json::to_string(&game).unwrap(), before);
    game = submit(&c, &game, &p, action(&members[0], 1, 0, 0)).unwrap();
    let p = game.state.decisions.pending.iter().find(|p| matches!(&game.state.setup.tasks.get(&p.id), Some(SetupTask::Trucks { group: g }) if g == &group)).unwrap().clone();
    assert!(game.state.land.undistributed_trucks.contains_key(&group));
    game = submit(
        &c,
        &game,
        &p,
        action(
            &members[1],
            original.light - 1,
            original.medium,
            original.heavy,
        ),
    )
    .unwrap();
    assert!(!game.state.land.undistributed_trucks.contains_key(&group));
    assert_eq!(game.state.land.units[&members[0]].trucks.light, 1);
    assert_eq!(
        game.state.land.units[&members[1]].trucks,
        Trucks {
            light: original.light - 1,
            medium: original.medium,
            heavy: original.heavy
        }
    );
}

/// Cases: scen:59.53
#[test]
fn dummy_dump_domain_excludes_facilities_and_forged_choice_is_atomic() {
    let mut c = content();
    let d = c
        .scenario
        .supply
        .dummy_dumps
        .iter_mut()
        .find(|d| d.side == Side::Axis)
        .unwrap();
    d.location = Placement::HexesAny {
        hexes: vec!["A4829".into(), "A0102".into()],
    };
    let game = opened(&c);
    let p = game.state.decisions.pending.iter().find(|p| p.kind == KIND_DUMP && p.seat.side == Side::Axis &&
        matches!(&game.state.setup.tasks[&p.id], SetupTask::Dump { dump, .. } if game.state.logistics.dumps[dump].dummy)).unwrap().clone();
    let ActionSchema::Choice { options } = &p.space.schema else {
        panic!();
    };
    assert_eq!(
        options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
        vec!["A0102"]
    );
    let before = serde_json::to_string(&game).unwrap();
    assert!(matches!(
        submit(&c, &game, &p, json!("A4829")).unwrap_err(),
        Rejection::Illegal { message } if message.contains("expected one of the listed option ids")
    ));
    assert_eq!(serde_json::to_string(&game).unwrap(), before);
    let game = submit(&c, &game, &p, json!("A0102")).unwrap();
    assert_eq!(game.state.setup.dump_locations.len(), 1);
}

/// Cases: scen:60.37, land:3.6
#[test]
fn convoy_planning_barrier_stays_in_setup_after_both_close_paths() {
    let c = content();
    let game = finish(&c, opened(&c));
    assert!(game.state.setup.closed);
    assert!(game.state.logistics.convoys_initialized);
    assert_eq!(game.state.cursor.block, crate::seq::Block::Setup);
    assert!(
        game.state
            .decisions
            .pending
            .iter()
            .any(|p| p.kind == "cna.logistics.convoy.plan:1" && p.secrecy == Secrecy::Secret)
    );
    assert!(
        Cna::dev()
            .view(&c, &game.state, Perspective::Side(Side::Commonwealth))
            .pending
            .iter()
            .all(|p| !p.kind.starts_with(crate::logistics::convoys::PREFIX))
    );
    let mut no_choices = State::new(&c).unwrap();

    for u in no_choices.land.units.values_mut() {
        if matches!(u.location, Location::AwaitingSetup { .. }) {
            u.location = Location::Hex {
                hex: if u.side == Side::Axis {
                    "C4020".into()
                } else {
                    "C4021".into()
                },
            };
        }
    }
    no_choices.land.undistributed_trucks.clear();
    no_choices.logistics.truck_pools.clear();
    no_choices.air.forces.clear();
    for d in no_choices.logistics.dumps.values_mut() {
        if matches!(d.location, DumpLocation::AwaitingSetup { .. }) {
            d.location = DumpLocation::Hex {
                hex: "C4020".into(),
            };
        }
    }

    let mut rng = CampaignRng::from_seed([4; 32]);
    let mut events = vec![];
    enter(
        &c,
        &mut no_choices,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert!(!no_choices.logistics.convoys_initialized);
    super::finish(
        &c,
        &mut no_choices,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert!(no_choices.logistics.convoys_initialized);
    assert!(
        no_choices
            .decisions
            .pending
            .iter()
            .any(|p| p.kind == "cna.logistics.convoy.plan:1")
    );
}

/// Cases: scen:59.2, land:9.12, land:9.31
#[test]
fn a_friendly_setup_choice_revises_other_groups_and_rejects_the_stale_answer() {
    let mut c = content();
    let mut state = State::new(&c).unwrap();
    let ids: Vec<_> = state
        .units_of(Side::Commonwealth)
        .filter(|u| {
            c.units.units[&u.id].stacking_points == Some(1)
                && !c.units.units[&u.id].sheet.contains("garrison")
                && c.units.units[&u.id]
                    .class
                    .as_ref()
                    .is_some_and(|id| c.units.classes[id].unit_type == "infantry")
        })
        .take(9)
        .map(|u| u.id.clone())
        .collect();
    assert_eq!(ids.len(), 9);
    for u in state.land.units.values_mut() {
        u.location = Location::NotArrived;
        u.detached = true;
    }
    state.land.undistributed_trucks.clear();
    state.logistics.dumps.clear();
    state.logistics.truck_pools.clear();
    state.air.forces.clear();
    for id in &ids[..7] {
        state.land.units.get_mut(id).unwrap().location = Location::Hex {
            hex: "E1730".into(),
        };
    }
    let file = c
        .scenario
        .land
        .iter_mut()
        .find(|f| f.file.side == Some(Side::Commonwealth))
        .unwrap();
    for (g, id) in file.groups.iter_mut().take(2).zip(&ids[7..]) {
        g.placement = Placement::HexesAny {
            hexes: vec!["E1730".into(), "E1830".into()],
        };
        let u = state.land.units.get_mut(id).unwrap();
        u.setup_group = Some(g.id.clone());
        u.location = Location::AwaitingSetup {
            group: g.id.clone(),
        };
    }
    let game = Game {
        state,
        rng: CampaignRng::from_seed([5; 32]).state(),
    };
    let mut game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
        .unwrap()
        .game;
    let first = game.state.decisions.pending[0].clone();
    let old = game.state.decisions.pending[1].clone();
    game = submit(&c, &game, &first, json!("E1730")).unwrap();
    let revised = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.id == old.id)
        .unwrap()
        .clone();
    assert_eq!(revised.revision, old.revision + 1);
    let ActionSchema::Choice { options } = &revised.space.schema else {
        panic!()
    };
    assert_eq!(
        options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
        vec!["E1830"]
    );
    assert!(matches!(
        submit(&c, &game, &old, json!("E1730")),
        Err(Rejection::StaleRevision { .. })
    ));
    let before = serde_json::to_value(&game).unwrap();
    assert!(submit(&c, &game, &revised, json!("E1730")).is_err());
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    submit(&c, &game, &revised, json!("E1830")).unwrap();
}

/// Cases: scen:59.43, scen:59.44, land:3.62
#[test]
fn starting_pool_splits_keep_identity_counts_and_private_locations_across_recovery() {
    let c = content();
    let mut initial = State::new(&c).unwrap();
    for u in initial.land.units.values_mut() {
        u.location = Location::NotArrived;
    }
    initial.land.undistributed_trucks.clear();
    initial.logistics.dumps.clear();
    initial.air.forces.clear();
    let original = initial
        .logistics
        .truck_pools
        .iter()
        .find(|p| matches!(&p.placement,Placement::City{city} if city=="cairo"))
        .unwrap()
        .clone();
    initial
        .logistics
        .truck_pools
        .retain(|p| p.id == original.id);
    let game = Game {
        state: initial,
        rng: CampaignRng::from_seed([5; 32]).state(),
    };
    let game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
        .unwrap()
        .game;
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == crate::setup::KIND_POOL)
        .unwrap()
        .clone();
    assert_eq!(p.seat.role, Role::Logistics);
    let before = serde_json::to_value(&game).unwrap();
    assert!(
        submit(
            &c,
            &game,
            &p,
            json!({"destination":"E1730","light":2147483647,"medium":2147483647,"heavy":2147483647})
        )
        .is_err()
    );
    assert!(
        submit(
            &c,
            &game,
            &p,
            json!({"destination":"A0101","light":0,"medium":1,"heavy":0})
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    let partial = submit(
        &c,
        &game,
        &p,
        json!({"destination":"E1730","light":0,"medium":1,"heavy":0}),
    )
    .unwrap();
    assert!(!partial.state.setup.closed);
    assert_eq!(partial.state.logistics.truck_pools.len(), 2);
    assert!(
        partial
            .state
            .logistics
            .truck_pools
            .iter()
            .all(|p| p.location.is_none())
    );
    assert_eq!(
        partial
            .state
            .logistics
            .truck_pools
            .iter()
            .find(|p| p.id == original.id)
            .unwrap()
            .trucks
            .medium,
        original.trucks.medium - 1
    );
    let fresh = partial
        .state
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id != original.id)
        .unwrap()
        .id
        .clone();
    assert!(partial.state.logistics.truck_pool_ids.contains(&fresh));
    let restored = serde_json::from_value(serde_json::to_value(&partial).unwrap()).unwrap();
    let complete = finish(&c, partial);
    let resumed = finish(&c, restored);
    assert_eq!(
        serde_json::to_value(&complete).unwrap(),
        serde_json::to_value(&resumed).unwrap()
    );
    assert_eq!(
        complete
            .state
            .logistics
            .truck_pools
            .iter()
            .map(|p| p.trucks.medium)
            .sum::<i32>(),
        original.trucks.medium
    );
    assert!(
        complete
            .state
            .logistics
            .truck_pools
            .iter()
            .all(|p| p.location.is_some())
    );
    assert!(complete.state.setup.pool_locations.is_empty());
}

/// Cases: scen:59.45, airlog:53.11, airlog:54.2
#[test]
fn optional_initial_cargo_shares_capacity_with_troops_and_never_spends_dump_supplies() {
    use cna_tables::airlog::{supply::SupplyType, trucks::TruckType};
    let c = content();
    let mut initial = State::new(&c).unwrap();
    let unit = initial
        .land
        .units
        .values()
        .find(|u| {
            u.setup_group.is_some()
                && c.units.units[&u.id]
                    .class
                    .as_ref()
                    .is_some_and(|id| c.units.classes[id].unit_type == "infantry")
        })
        .unwrap()
        .id
        .clone();
    for u in initial.land.units.values_mut() {
        u.location = Location::NotArrived;
    }
    let target = initial.land.units.get_mut(&unit).unwrap();
    target.location = Location::Hex {
        hex: "A0101".into(),
    };
    target.trucks = Trucks {
        light: 2,
        medium: 0,
        heavy: 0,
    };
    initial.land.undistributed_trucks.clear();
    initial.logistics.dumps.clear();
    initial.logistics.truck_pools.clear();
    initial.air.forces.clear();
    let side = target_side(&initial, &unit);
    let mut game = evaluate(
        &Cna::dev(),
        &c,
        &Game {
            state: initial,
            rng: CampaignRng::from_seed([5; 32]).state(),
        },
        &Command::Advance,
    )
    .unwrap()
    .game;
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == crate::setup::KIND_PRELOAD)
        .unwrap()
        .clone();
    assert!(p.space.pass.is_some());
    game = submit(&c, &game, &p, json!("load")).unwrap();
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == crate::setup::KIND_PRELOAD)
        .unwrap()
        .clone();
    let cap = c
        .tables
        .airlog
        .truck_characteristics
        .truck(TruckType::Light);
    let before = serde_json::to_value(&game).unwrap();
    assert!(submit(&c,&game,&p,json!({"light":{"stores":2*cap.supply_capacity(SupplyType::Stores),"fuel":2*cap.supply_capacity(SupplyType::Fuel)}})).is_err());
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    let enemy_before = Cna::dev().observe(&c, &game.state, Perspective::Side(side.opponent()));
    game = submit(
        &c,
        &game,
        &p,
        json!({"light":{"stores":2*cap.supply_capacity(SupplyType::Stores)}}),
    )
    .unwrap();
    assert_eq!(
        game.state.logistics.unit_supply[&unit].carried.stores,
        2 * cap.supply_capacity(SupplyType::Stores)
    );
    assert_eq!(
        Cna::dev().observe(&c, &game.state, Perspective::Side(side.opponent())),
        enemy_before
    );
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == crate::setup::KIND_PRELOAD)
        .unwrap()
        .clone();
    game = submit(&c, &game, &p, json!("motorize")).unwrap();
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == crate::setup::KIND_PRELOAD)
        .unwrap()
        .clone();
    assert!(submit(&c, &game, &p, json!({"light":1,"medium":0,"heavy":0})).is_err());
    assert_eq!(
        game.state.land.units[&unit].transport_trucks,
        Trucks::default()
    );
    game = submit(&c, &game, &p, json!({"light":0,"medium":0,"heavy":0})).unwrap();
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == crate::setup::KIND_PRELOAD)
        .unwrap()
        .clone();
    game = submit(&c, &game, &p, Value::Null).unwrap();
    assert!(!game.state.setup.closed);
    game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
        .unwrap()
        .game;
    assert!(game.state.setup.closed);
    assert!(game.state.logistics.dumps.is_empty());
}
fn target_side(state: &State, id: &UnitId) -> Side {
    state.land.units[id].side
}

/// Cases: scen:59.2, scen:59.42, scen:59.43, scen:59.44, scen:59.45, land:3.62
#[test]
fn setup_context_identifies_each_owner_asset_and_survives_checkpointing() {
    let c = content();
    let mut game = opened(&c);
    let mut seen = BTreeSet::new();
    for _ in 0..500 {
        if game.state.setup.closed {
            break;
        }
        if game.state.decisions.pending.is_empty() {
            game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
                .unwrap()
                .game;
            continue;
        }
        let p = game.state.decisions.pending[0].clone();
        let context = p.space.to_json_schema()["x-context"].clone();
        assert!(context.is_object(), "missing context for {}", p.kind);
        match &game.state.setup.tasks[&p.id] {
            SetupTask::Air { force, .. } => assert_eq!(context["force"], json!(force)),
            SetupTask::Unit { unit, .. } => {
                assert_eq!(context["unit"], json!(unit));
                assert_eq!(
                    context["group"],
                    json!(game.state.land.units[unit].setup_group)
                );
            }
            SetupTask::Dump { dump, .. } => assert_eq!(context["dump"], json!(dump)),
            SetupTask::Trucks { group } => {
                assert_eq!(context["group"], json!(group));
                assert_eq!(context["pool"], json!(format!("first-line:{group}")));
            }
            SetupTask::Pool { pool, .. } => assert_eq!(context["pool"], json!(pool)),
            SetupTask::Preload { asset, .. } => match asset {
                super::super::preload::Asset::Unit { unit } => {
                    assert_eq!(context["unit"], json!(unit))
                }
                super::super::preload::Asset::Pool { pool } => {
                    assert_eq!(context["pool"], json!(pool))
                }
            },
        }
        let enemy = Cna::dev().view(&c, &game.state, Perspective::Side(p.seat.side.opponent()));
        assert!(enemy.pending.iter().all(|q| q.id != p.id.to_string()));
        let restored: Game<Cna> =
            serde_json::from_value(serde_json::to_value(&game).unwrap()).unwrap();
        assert_eq!(
            restored.state.decisions.pending[0].space.to_json_schema()["x-context"],
            context
        );
        seen.insert(p.kind.clone());
        game = submit(&c, &game, &p, first(&p.space.schema)).unwrap();
    }
    assert!(game.state.setup.closed);
    assert!(seen.contains(crate::setup::KIND_POOL));
    assert!(seen.contains(crate::setup::KIND_PRELOAD));
}

/// Cases: scen:59.2, scen:59.35, scen:59.43, land:3.6
#[test]
fn enemy_initial_air_and_pool_holdings_are_indistinguishable_during_setup() {
    let c = content();
    let game = opened(&c);
    for observer in [Side::Axis, Side::Commonwealth] {
        let mut other = game.state.clone();
        for pool in &mut other.logistics.truck_pools {
            if pool.side != observer {
                pool.trucks.light += 1;
                pool.cargo.fuel += 1;
            }
        }
        for (id, force) in &mut other.air.forces {
            let side = if id == "axis" {
                Side::Axis
            } else {
                Side::Commonwealth
            };
            if side != observer {
                for planes in force.planes.values_mut() {
                    planes.total += 1;
                }
            }
        }
        crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &game.state, &other, observer);
    }
}

/// Cases: scen:59.45, airlog:53.11, airlog:54.2
#[test]
fn optional_cargo_and_motorization_operations_can_be_cancelled_without_changes() {
    let c = content();
    let mut game = finish(&c, opened(&c));
    game.state.setup.closed = false;
    game.state.decisions.pending.clear();
    game.state.setup.tasks.clear();
    game.state.setup.preload_started = false;
    let unit = game
        .state
        .land
        .units
        .values()
        .find(|u| {
            u.setup_group.is_some()
                && u.trucks.total() > 0
                && crate::land::formation::class(&c, &u.id)
                    .is_some_and(|cl| cl.unit_type == "infantry")
        })
        .unwrap()
        .id
        .clone();
    let mut rng = CampaignRng::from_state(&game.rng);
    let mut events = Vec::new();
    let mut cx = cna_core::engine::Cx {
        rng: &mut rng,
        events: &mut events,
    };
    super::super::preload::start(&c, &mut game.state, &mut cx).unwrap();
    for operation in ["load", "motorize"] {
        let p = game
            .state
            .decisions
            .pending
            .iter()
            .find(|p| {
                p.space
                    .context
                    .as_ref()
                    .is_some_and(|x| x["unit"] == unit.as_str())
            })
            .unwrap()
            .clone();
        game = submit(&c, &game, &p, json!(operation)).unwrap();
        let p = game
            .state
            .decisions
            .pending
            .iter()
            .find(|p| {
                p.space
                    .context
                    .as_ref()
                    .is_some_and(|x| x["unit"] == unit.as_str())
            })
            .unwrap()
            .clone();
        assert!(p.space.pass.is_some());
        let before = game.state.land.units[&unit].clone();
        let supply = game.state.logistics.unit_supply.get(&unit).cloned();
        game = submit(&c, &game, &p, Value::Null).unwrap();
        assert_eq!(game.state.land.units[&unit], before);
        assert_eq!(game.state.logistics.unit_supply.get(&unit).cloned(), supply);
    }
}

/// Cases: scen:59.2, land:3.62
/// Interpretations: interp:scen-0005
#[test]
fn every_placement_at_real_setup_closure_has_an_owner_unit_update() {
    let c = content();
    let mut game = opened(&c);
    for _ in 0..500 {
        if game.state.decisions.pending.is_empty() {
            break;
        }
        let p = game.state.decisions.pending[0].clone();
        game = submit(&c, &game, &p, first(&p.space.schema)).unwrap();
    }
    assert!(game.state.decisions.pending.is_empty());
    assert!(!game.state.setup.closed);
    let placed = game.state.setup.unit_locations.clone();
    assert!(!placed.is_empty());
    let t = evaluate(&Cna::dev(), &c, &game, &Command::Advance).unwrap();
    assert!(t.game.state.setup.closed);
    for (id, location) in placed {
        let side = t.game.state.land.units[&id].side;
        let expected = Cna::dev()
            .view(&c, &t.game.state, Perspective::Side(side))
            .units[&id.to_string()]
            .clone();
        assert_eq!(t.game.state.land.units[&id].location, location);
        let own: Vec<_> = t
            .events
            .iter()
            .filter(|e| Perspective::Side(side).can_see(&e.audience))
            .filter_map(|e| match &e.event {
                GameEvent::UnitUpdated { unit } if unit.id == id.as_str() => Some(unit),
                _ => None,
            })
            .collect();
        assert_eq!(
            own,
            vec![&expected],
            "missing or duplicate closure update for {id}"
        );
        let operator: Vec<_> = t
            .events
            .iter()
            .filter(|e| Perspective::Operator.can_see(&e.audience))
            .filter_map(|e| match &e.event {
                GameEvent::UnitUpdated { unit } if unit.id == id.as_str() => Some(unit),
                _ => None,
            })
            .collect();
        assert_eq!(
            operator,
            vec![&expected],
            "missing or duplicate operator closure update for {id}"
        );
        // The other side gets the placed counter's face, nothing more (land:3.62).
        for e in t
            .events
            .iter()
            .filter(|e| Perspective::Side(side.opponent()).can_see(&e.audience))
        {
            if let GameEvent::UnitUpdated { unit } = &e.event
                && unit.id == id.as_str()
            {
                crate::testkit::assert_face(&serde_json::to_value(unit).unwrap());
            }
        }
    }
}

/// Cases: scen:59.2, land:3.6, land:3.62, airlog:54.11
/// Interpretations: interp:scen-0005
#[test]
fn closure_events_disclose_faces_only_and_sort_dump_markers_by_public_id() {
    let c = content();
    let mut state = State::new(&c).unwrap();
    state.setup.started = true;
    state.cursor.entered = true;
    state.logistics.convoys_initialized = true;
    state.land.undistributed_trucks.clear();
    state.logistics.dumps.clear();
    state.logistics.truck_pools.clear();
    state.air.forces.clear();
    let ids: Vec<_> = state
        .units_of(Side::Axis)
        .take(2)
        .map(|u| u.id.clone())
        .collect();
    for u in state.land.units.values_mut() {
        u.location = Location::NotArrived;
    }
    for id in &ids {
        state.land.units.get_mut(id).unwrap().location = Location::AwaitingSetup {
            group: "event-test".into(),
        };
    }
    state.setup.unit_locations.insert(
        ids[0].clone(),
        Location::Hex {
            hex: "A0101".into(),
        },
    );
    state.setup.placement_order.insert(ids[0].clone(), 1);
    for (id, marker) in [("private-first", "dump-9"), ("private-second", "dump-2")] {
        state.logistics.dumps.insert(
            id.into(),
            crate::state::Dump {
                id: id.into(),
                marker: marker.into(),
                side: Side::Axis,
                location: DumpLocation::AwaitingSetup {
                    placement: cna_content::scenario::Placement::Hex {
                        hex: "A0102".into(),
                    },
                },
                supplies: cna_content::scenario::Supplies::default(),
                active: true,
                dummy: false,
            },
        );
        state.setup.dump_locations.insert(
            id.into(),
            Location::Hex {
                hex: "A0102".into(),
            },
        );
    }
    let a = Game::<Cna> {
        state,
        rng: CampaignRng::from_seed([5; 32]).state(),
    };
    let mut b = a.clone();
    b.state.setup.unit_locations.insert(
        ids[1].clone(),
        Location::Hex {
            hex: "A0101".into(),
        },
    );
    b.state.setup.placement_order.insert(ids[1].clone(), 2);
    // The second unit is attached to the first, so its counter is not on the map (land:4.25).
    let second = b.state.land.units.get_mut(&ids[1]).unwrap();
    second.attached_to = Some(ids[0].clone());
    second.detached = false;
    b.state.land.units.get_mut(&ids[0]).unwrap().toe =
        Some(cna_content::units::Toe::Under { under: 1 });
    for dump in b.state.logistics.dumps.values_mut() {
        dump.dummy = true;
        dump.supplies.fuel = 99;
    }
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        &c,
        &a,
        &b,
        &Command::Advance,
        Side::Commonwealth,
    );
    let t = evaluate(&Cna::dev(), &c, &b, &Command::Advance).unwrap();
    let seen: Vec<_> = t
        .events
        .iter()
        .filter(|e| Perspective::Side(Side::Commonwealth).can_see(&e.audience))
        .collect();
    let stacks: Vec<_> = seen
        .iter()
        .filter_map(|e| match &e.event {
            GameEvent::StackUpdated { stack } if stack.side == Side::Axis => Some(stack),
            _ => None,
        })
        .collect();
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].hex, "A0101");
    assert_eq!(stacks[0].unit_ids, vec![ids[0].to_string()]);
    assert_eq!(stacks[0].visible_count, Some(1));
    for e in &seen {
        if let GameEvent::UnitUpdated { unit } = &e.event
            && unit.side == Side::Axis
        {
            assert_eq!(unit.id, ids[0].as_str());
            crate::testkit::assert_face(&serde_json::to_value(unit).unwrap());
        }
    }
    let markers: Vec<_> = seen
        .iter()
        .filter_map(|e| match &e.event {
            GameEvent::MarkerPlaced { marker } => Some(marker),
            _ => None,
        })
        .collect();
    assert_eq!(
        markers.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["dump-2", "dump-9"]
    );
    assert!(
        markers
            .iter()
            .all(|m| m.label.as_deref() == Some(m.id.as_str()))
    );
}
