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
        let p = game.state.decisions.pending[0].clone();
        game = submit(c, &game, &p, first(&p.space.schema)).unwrap();
    }
    panic!("setup failed to finish");
}

/// Cases: scen:59.2, scen:59.42, land:3.62
/// Interpretations: interp:scen-0005
#[test]
fn private_choices_publish_presence_only_after_the_entire_window_and_preserve_trucks() {
    let c = content();
    let initial = State::new(&c).unwrap();
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
    let game = opened(&c);
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
    let action = first(&p.space.schema);
    let enemy_before = Cna::dev().view(&c, &game.state, Perspective::Side(Side::Commonwealth));
    let game = submit(&c, &game, &p, action).unwrap();
    assert!(!game.state.setup.closed);
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
    assert!(stack.unit_ids.is_empty() && stack.visible_count.is_none());
    assert!(!enemy.units.contains_key(unit.as_str()));
}

/// Cases: scen:59.2, land:8.13
/// Interpretations: interp:scen-0005
#[test]
fn later_opposing_collision_rejects_only_that_answer_and_can_retry() {
    let mut c = content();
    let groups: Vec<_> = c
        .scenario
        .land
        .iter_mut()
        .map(|f| {
            let g = &mut f.groups[0];
            g.placement = Placement::HexesAny {
                hexes: vec!["A0101".into(), "A0102".into()],
            };
            (f.file.side.unwrap(), g.id.clone())
        })
        .collect();
    let mut game = opened(&c);
    let requests: Vec<_> = groups.iter().map(|(side, group)| {
        game.state.decisions.pending.iter().find(|p| p.kind == KIND_UNIT && p.seat.side == *side &&
            matches!(&game.state.setup.tasks[&p.id], SetupTask::Unit { unit, .. } if game.state.land.units[unit].setup_group.as_ref() == Some(group))).unwrap().clone()
    }).collect();
    game = submit(&c, &game, &requests[0], json!("A0101")).unwrap();
    let before = serde_json::to_string(&game).unwrap();
    let error = submit(&c, &game, &requests[1], json!("A0101")).unwrap_err();
    assert_eq!(error, illegal("not a legal setup destination"));
    assert_eq!(serde_json::to_string(&game).unwrap(), before);
    assert!(
        game.state
            .decisions
            .pending
            .iter()
            .any(|p| p.id == requests[1].id)
    );
    game = submit(&c, &game, &requests[1], json!("A0102")).unwrap();
    assert!(!game.state.setup.closed);
    assert_eq!(game.state.setup.unit_locations.len(), 2);
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
    assert_eq!(
        submit(&c, &game, &p, json!("A4829")).unwrap_err(),
        illegal("not a legal setup destination")
    );
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
    assert!(no_choices.logistics.convoys_initialized);
    assert!(
        no_choices
            .decisions
            .pending
            .iter()
            .any(|p| p.kind == "cna.logistics.convoy.plan:1")
    );
}
