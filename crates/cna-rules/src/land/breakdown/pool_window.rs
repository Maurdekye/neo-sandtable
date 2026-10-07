//! Convoy stops share the mandatory loss kind while keeping pool bookkeeping additive.
use super::{
    overflow,
    pool_losses::{self, PoolLossPlan, PoolOutcome},
    pools,
};
use crate::{
    CnaContent, State,
    state::Pending,
    steps::{illegal, open},
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::SeatId,
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolWindow {
    pub pool: String,
    pub held: Vec<Pending>,
    pub outcomes: VecDeque<PoolOutcome>,
    pub submitted: Option<PoolLossPlan>,
}
#[derive(Debug, Clone)]
pub enum PoolFinish {
    Complete,
    Waiting,
    /// The caller applies exact cargo, cohort and water consequences on its transaction,
    /// then calls finish_pool again before continuing the convoy's next accepted order.
    Loss {
        outcome: Box<PoolOutcome>,
        plan: Box<PoolLossPlan>,
    },
}
fn field(name: &str, schema: ActionSchema) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: name.replace('_', " "),
        schema,
        optional: false,
    }
}
fn record(names: &[&str], schema: ActionSchema) -> ActionSchema {
    ActionSchema::Record {
        fields: names.iter().map(|n| field(n, schema.clone())).collect(),
    }
}
pub(super) fn space(o: &PoolOutcome) -> ActionSpace {
    let count = ActionSchema::Integer {
        min: 0,
        max: i64::from(i32::MAX),
    };
    let cargo = record(&["fuel", "ammo", "stores", "water"], count.clone());
    let packing = record(&["light", "medium", "heavy"], cargo);
    let mut partition = ["working", "origin", "destination"]
        .iter()
        .map(|n| field(n, packing.clone()))
        .collect::<Vec<_>>();
    partition.extend(
        [
            "origin_tank_fuel_tenths",
            "destination_tank_fuel_tenths",
            "origin_activity_water",
            "destination_activity_water",
        ]
        .iter()
        .map(|n| field(n, count.clone())),
    );
    let cohort = ActionSchema::Record {
        fields: vec![
            field(
                "id",
                ActionSchema::Text {
                    min_length: 1,
                    max_length: 512,
                },
            ),
            field(
                "count",
                ActionSchema::Integer {
                    min: 1,
                    max: i64::from(i32::MAX),
                },
            ),
        ],
    };
    let choices = ActionSchema::List {
        item: Box::new(cohort),
        min: 0,
        max: o.group.assets.len() as u32,
    };
    ActionSpace::new(ActionSchema::Record {
        fields: vec![
            field(
                "pool",
                ActionSchema::Text {
                    min_length: 1,
                    max_length: 512,
                },
            ),
            field("losses", choices.clone()),
            field("at_origin", choices),
            field("partition", ActionSchema::Record { fields: partition }),
        ],
    })
    .with_context(json!({"pool_breakdown":o}))
}
/// Save only an own-validated allocation. Dice, enemy origin constraints and losses are deferred.
/// Cases: land:21.35, land:21.36, land:21.41, land:21.43
pub(super) fn answer(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    action: &Value,
) -> Result<String, Rejection> {
    let w = s
        .land
        .breakdown
        .window
        .pool
        .as_ref()
        .ok_or_else(|| illegal("pool breakdown is no longer open"))?;
    let o = w
        .outcomes
        .front()
        .ok_or_else(|| illegal("pool roll is no longer open"))?;
    if p.seat != SeatId::new(o.group.side, Role::Logistics) {
        return Err(illegal("these broken pool trucks belong to another seat"));
    }
    let plan: PoolLossPlan = serde_json::from_value(action.clone())
        .map_err(|_| illegal("provide selected pool cohorts and conserved holdings"))?;
    pool_losses::validate(c, s, o, &plan)?;
    s.land.breakdown.window.pool.as_mut().unwrap().submitted = Some(plan);
    Ok("Own pool breakdown allocation recorded.".into())
}
fn roll(
    c: &CnaContent,
    s: &mut State,
    id: &str,
    stopped: &pools::PoolStop,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<Vec<PoolOutcome>, EngineError> {
    let groups = pools::check_groups(c, s, id, stopped)?;
    let (b, _) = pools::histories(s, id)?;
    s.land.breakdown.pools.insert(id.into(), b);
    let mut outcomes = vec![];
    for group in groups {
        let first = group.assets.first().ok_or_else(overflow)?;
        let h = &s.land.breakdown.pools[id].truck_histories[&first.cohort];
        let bp = h
            .base_quarters
            .checked_add(if first.equipment == super::Equipment::LightTruck {
                h.light_extra_quarters
            } else {
                0
            })
            .ok_or_else(overflow)?;
        for a in &group.assets {
            s.land
                .breakdown
                .pools
                .get_mut(id)
                .unwrap()
                .truck_histories
                .get_mut(&a.cohort)
                .ok_or_else(overflow)?
                .checked = Some(group.column);
        }
        if (group.column as i64) + i64::from(group.shift) < 1 {
            continue;
        }
        let d = cx.rng.two_dice_reading();
        cx.emit(
            EngineEvent::new(
                Audience::Side(group.side),
                GameEvent::DiceRolled {
                    purpose: format!(
                        "Own convoy breakdown, BP band {}, shift {}",
                        group.column, group.shift
                    ),
                    dice: vec![d.tens.value(), d.units.value()],
                    reading: Some(d.value()),
                    rule: Some("land:21.34".into()),
                },
            )
            .at(stopped.destination.clone()),
        );
        let points = group.assets.iter().map(|a| a.points).collect::<Vec<_>>();
        let (percent, broken) =
            super::core::losses(&c.tables.land.breakdown, bp, group.shift, &points, d)?;
        if broken == 0 {
            continue;
        }
        if let Some(e) = &stopped.motion.origin_gap {
            if strict {
                return Err(e.clone());
            }
            cx.emit(EngineEvent::new(Audience::Side(group.side),GameEvent::Note{text:"Convoy breakdown origin placement is unassessed because its map data is incomplete (land:21.41).".into()}).at(stopped.motion.origin.clone()));
        }
        outcomes.push(PoolOutcome {
            pool: id.into(),
            group,
            percent,
            broken,
            origin: stopped.motion.origin.clone(),
            destination: stopped.destination.clone(),
            require_origin: stopped.motion.origin_required,
        });
    }
    Ok(outcomes)
}
/// Adjudicate one completed real pool stop before the next convoy order proceeds.
/// Answers remain record-only; validated consequences are returned to the logistics caller.
/// Cases: land:21.22, land:21.24, land:21.25, land:21.28, land:21.31, land:21.41, land:21.43
pub fn finish_pool(
    c: &CnaContent,
    s: &mut State,
    id: &str,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<PoolFinish, EngineError> {
    pools::carrier(s, id)?;
    if s.land.breakdown.window.parked || !s.land.breakdown.window.outcomes.is_empty() {
        return Err(EngineError::Invariant {
            detail: "unit breakdown must finish before opening a convoy loss window".into(),
        });
    }
    if let Some(w) = &s.land.breakdown.window.pool {
        if w.pool != id {
            return Err(EngineError::Invariant {
                detail: "finish the current convoy breakdown before starting another".into(),
            });
        }
    } else {
        if s.land
            .breakdown
            .pools
            .get(id)
            .is_none_or(|b| b.stopped.is_empty())
        {
            return Ok(PoolFinish::Complete);
        }
        s.land.breakdown.window.pool = Some(PoolWindow {
            pool: id.into(),
            held: std::mem::take(&mut s.decisions.pending),
            outcomes: VecDeque::new(),
            submitted: None,
        });
    }
    loop {
        if let Some(plan) = s
            .land
            .breakdown
            .window
            .pool
            .as_mut()
            .unwrap()
            .submitted
            .take()
        {
            let outcome = s
                .land
                .breakdown
                .window
                .pool
                .as_mut()
                .unwrap()
                .outcomes
                .pop_front()
                .ok_or_else(overflow)?;
            pool_losses::validate(c, s, &outcome, &plan).map_err(|e| match e {
                Rejection::Engine(e) => e,
                _ => EngineError::Invariant {
                    detail: "accepted convoy breakdown holdings changed before adjudication".into(),
                },
            })?;
            return Ok(PoolFinish::Loss {
                outcome: Box::new(outcome),
                plan: Box::new(plan),
            });
        }
        if let Some(outcome) = s
            .land
            .breakdown
            .window
            .pool
            .as_ref()
            .unwrap()
            .outcomes
            .front()
            .cloned()
        {
            if pool_losses::plan(c, s, &outcome).is_none() {
                return Err(EngineError::Unsupported {
                    case: "land:21.43".into(),
                    detail:
                        "no exact conserved cargo/reserve allocation exists for the convoy holdings"
                            .into(),
                });
            }
            if !s
                .decisions
                .pending
                .iter()
                .any(|p| p.kind == super::window::KIND)
            {
                open(
                    s,
                    cx,
                    SeatId::new(outcome.group.side, Role::Logistics),
                    super::window::KIND,
                    "Allocate own broken convoy trucks and conserved cargo.".into(),
                    &["land:21.35", "land:21.36", "land:21.41", "land:21.43"],
                    Trigger::Triggered,
                    Secrecy::Secret,
                    space(&outcome),
                );
            }
            return Ok(PoolFinish::Waiting);
        }
        let stopped = s
            .land
            .breakdown
            .pools
            .get_mut(id)
            .and_then(|b| b.stopped.pop_front());
        if let Some(stopped) = stopped {
            let outcomes = roll(c, s, id, &stopped, strict, cx)?;
            s.land
                .breakdown
                .window
                .pool
                .as_mut()
                .unwrap()
                .outcomes
                .extend(outcomes);
            continue;
        }
        let mut w = s.land.breakdown.window.pool.take().unwrap();
        s.decisions.pending.append(&mut w.held);
        return Ok(PoolFinish::Complete);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cna, state::Location};
    use cna_content::units::Trucks;
    use cna_core::{
        decision::DecisionResponse,
        dice::CampaignRng,
        engine::{Command, Game, Ruleset, evaluate},
        visibility::Perspective,
    };
    use cna_tables::land::weather::WeatherKind;
    fn stopped() -> (CnaContent, State, String) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let mut p = s.logistics.truck_pools[0].clone();
        p.id = "convoy-stop".into();
        p.side = cna_protocol::Side::Commonwealth;
        p.location = Some(Location::Hex {
            hex: "C4021".into(),
        });
        p.trucks = Trucks {
            light: 0,
            medium: 30,
            heavy: 0,
        };
        p.cargo = Default::default();
        p.tank_fuel = Default::default();
        p.activity_water = Default::default();
        s.logistics.truck_pools = vec![p];
        pools::record_edge(
            &mut s,
            "convoy-stop",
            &"C4020".into(),
            280,
            8,
            WeatherKind::Normal,
        )
        .unwrap();
        pools::stop(&mut s, "convoy-stop", &"C4021".into());
        (c, s, "convoy-stop".into())
    }
    /// Cases: land:21.22, land:21.24, land:21.31, land:21.34, land:21.43
    #[test]
    fn pool_roll_choice_and_handoff_resume_across_checkpoint_without_unit_mutation() {
        let (c, initial, id) = stopped();
        let units = serde_json::to_value(&initial.land.units).unwrap();
        let mut found = None;
        for seed in 0..64u8 {
            let mut s = initial.clone();
            let mut rng = CampaignRng::from_seed([seed; 32]);
            let old = serde_json::to_value(rng.state()).unwrap();
            let mut events = vec![];
            let result = finish_pool(
                &c,
                &mut s,
                &id,
                false,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events,
                },
            )
            .unwrap();
            assert!(
                events
                    .iter()
                    .all(|e| !Perspective::Side(cna_protocol::Side::Axis).can_see(&e.audience))
            );
            assert!(
                events
                    .iter()
                    .filter(|e| matches!(e.event, GameEvent::DiceRolled { .. }))
                    .all(|e| e.hex.is_some() && e.unit_id.is_none())
            );
            assert_ne!(serde_json::to_value(rng.state()).unwrap(), old);
            if matches!(result, PoolFinish::Waiting) {
                found = Some((s, rng));
                break;
            }
        }
        let (s, rng) = found.expect("at least one real roll yields broken trucks");
        let game: Game<Cna> = Game {
            state: s,
            rng: rng.state(),
        };
        let p = &game.state.decisions.pending[0];
        assert_eq!(p.kind, super::super::window::KIND);
        assert!(p.space.pass.is_none());
        let request = Cna::dev()
            .pending(&c, &game.state)
            .into_iter()
            .find(|r| r.id == p.id)
            .unwrap();
        for seed in 0..32u8 {
            let mut owner_rng = CampaignRng::from_seed([seed; 32]);
            let action =
                crate::baseline::random_breakdown(&c, &game.state, &request, &mut owner_rng);
            assert!(!action.is_null());
            let response = Command::Respond(DecisionResponse {
                decision_id: p.id.clone(),
                seat: p.seat,
                decision_revision: p.revision,
                controller_epoch: 1,
                idempotency_key: format!("pool-loss-{seed}"),
                public_explanation: None,
                action,
            });
            let batch = evaluate(&Cna::dev(), &c, &game, &response).unwrap();
            assert_eq!(
                serde_json::to_value(&batch.game.rng).unwrap(),
                serde_json::to_value(&game.rng).unwrap()
            );
            assert!(batch.game.state.land.breakdown.markers.is_empty());
            if seed == 0 {
                let restored: State =
                    serde_json::from_value(serde_json::to_value(&batch.game.state).unwrap())
                        .unwrap();
                let mut a = batch.game.state;
                let mut b = restored;
                let mut ra = CampaignRng::from_state(&batch.game.rng);
                let mut rb = CampaignRng::from_state(&batch.game.rng);
                let mut ea = vec![];
                let mut eb = vec![];
                let aa = finish_pool(
                    &c,
                    &mut a,
                    &id,
                    false,
                    &mut Cx {
                        rng: &mut ra,
                        events: &mut ea,
                    },
                )
                .unwrap();
                let bb = finish_pool(
                    &c,
                    &mut b,
                    &id,
                    false,
                    &mut Cx {
                        rng: &mut rb,
                        events: &mut eb,
                    },
                )
                .unwrap();
                let PoolFinish::Loss { outcome, plan } = aa else {
                    panic!("accepted allocation must hand off once")
                };
                let PoolFinish::Loss {
                    outcome: bo,
                    plan: bp,
                } = bb
                else {
                    panic!("restored allocation must hand off once")
                };
                assert_eq!(
                    serde_json::to_value((&outcome, &plan)).unwrap(),
                    serde_json::to_value((&bo, &bp)).unwrap()
                );
                assert_eq!(serde_json::to_value(&a.land.units).unwrap(), units);
                // Consequences belong to the convoy owner and are applied in this same draft.
                // This check exercises window completion independently of that physical hook.
                assert!(matches!(
                    finish_pool(
                        &c,
                        &mut a,
                        &id,
                        false,
                        &mut Cx {
                            rng: &mut ra,
                            events: &mut ea
                        }
                    )
                    .unwrap(),
                    PoolFinish::Complete
                ));
            }
        }
    }
    /// Cases: land:3.62, land:21.35, land:21.41, land:21.43
    #[test]
    fn pool_choice_privacy_and_validation_do_not_consult_hidden_enemy_holdings() {
        let (c, initial, id) = stopped();
        let (s, rng) = (0..64u8)
            .find_map(|seed| {
                let mut s = initial.clone();
                let mut rng = CampaignRng::from_seed([seed; 32]);
                let mut events = vec![];
                matches!(
                    finish_pool(
                        &c,
                        &mut s,
                        &id,
                        false,
                        &mut Cx {
                            rng: &mut rng,
                            events: &mut events
                        }
                    )
                    .unwrap(),
                    PoolFinish::Waiting
                )
                .then_some((s, rng))
            })
            .expect("a rolled pool loss opens the private choice");
        let game: Game<Cna> = Game {
            state: s,
            rng: rng.state(),
        };
        let request = Cna::dev()
            .pending(&c, &game.state)
            .into_iter()
            .find(|p| p.kind == super::super::window::KIND)
            .unwrap();
        let plan = pool_losses::plan(
            &c,
            &game.state,
            game.state
                .land
                .breakdown
                .window
                .pool
                .as_ref()
                .unwrap()
                .outcomes
                .front()
                .unwrap(),
        )
        .unwrap();
        let command = |action| {
            Command::Respond(DecisionResponse {
                decision_id: request.id.clone(),
                seat: request.seat,
                decision_revision: request.revision,
                controller_epoch: 1,
                idempotency_key: "pool-privacy".into(),
                public_explanation: None,
                action,
            })
        };
        let first = command(serde_json::to_value(&plan).unwrap());
        let mut hidden = game.clone();
        let enemy = hidden
            .state
            .units_of(cna_protocol::Side::Axis)
            .next()
            .unwrap()
            .id
            .clone();
        hidden
            .state
            .land
            .units
            .get_mut(&enemy)
            .unwrap()
            .cohesion_quarters = -104;
        hidden
            .state
            .logistics
            .unit_supply
            .entry(enemy)
            .or_default()
            .ready_ammo = cna_core::quantity::AmmoPoints::new(0);
        crate::testkit::assert_indistinguishable(
            &Cna::dev(),
            &c,
            &game.state,
            &hidden.state,
            cna_protocol::Side::Commonwealth,
        );
        crate::testkit::assert_action_indistinguishable(
            &Cna::dev(),
            &c,
            &game,
            &hidden,
            &first,
            cna_protocol::Side::Commonwealth,
        );
        let mut alternate = plan.clone();
        alternate.at_origin = alternate.losses.clone();
        alternate.partition.origin = std::mem::take(&mut alternate.partition.destination);
        alternate.partition.origin_tank_fuel_tenths =
            std::mem::take(&mut alternate.partition.destination_tank_fuel_tenths);
        alternate.partition.origin_activity_water =
            std::mem::take(&mut alternate.partition.destination_activity_water);
        let second = command(serde_json::to_value(alternate).unwrap());
        crate::testkit::assert_actions_indistinguishable(
            &Cna::dev(),
            &c,
            (&game, &first),
            (&game, &second),
            cna_protocol::Side::Axis,
        );
        let mut bad = plan;
        bad.losses[0].count += 1;
        assert!(
            evaluate(
                &Cna::dev(),
                &c,
                &game,
                &command(serde_json::to_value(bad).unwrap())
            )
            .is_err()
        );
        assert!(
            game.state
                .land
                .breakdown
                .window
                .pool
                .as_ref()
                .unwrap()
                .submitted
                .is_none()
        );
    }
}
