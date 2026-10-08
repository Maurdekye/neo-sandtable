use super::*;
use crate::Cna;
use crate::seq::{Block, PLAYER_HALF};
use cna_content::scenario::Placement;
use cna_core::decision::DecisionResponse;
use cna_core::dice::CampaignRng;
use cna_core::engine::{Command, Game, Ruleset, evaluate};
use std::sync::OnceLock;

fn respond(p: &Pending, action: Value) -> Command {
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

fn checkpoint(game: &Game<Cna>) -> Game<Cna> {
    serde_json::from_value(serde_json::to_value(game).unwrap()).unwrap()
}

fn dispatched_finish(
    rules: Cna,
    state: &mut State,
    rng: &mut CampaignRng,
    events: &mut Vec<EngineEvent>,
) -> Result<(), EngineError> {
    rules.finish_step(content(), state, &mut Cx { rng, events })
}

/// Cases: airlog:48.0, airlog:53.12, land:3.6
#[test]
fn actual_dispatcher_entered_checkpoint_opens_once_in_both_halves_and_profiles() {
    for strict in [false, true] {
        for half in [Half::A, Half::B] {
            let rules = Cna { strict };
            let (mut state, _) = fixture();
            state.cursor.half = Some(half);
            state.cursor.entered = true;
            let actor = state.cursor.phasing(state.turn.player_a).unwrap();
            let game: Game<Cna> = Game {
                state,
                rng: CampaignRng::from_seed([8; 32]).state(),
            };
            let opened = evaluate(&rules, content(), &game, &Command::Advance).unwrap();
            assert_eq!(opened.game.state.cursor.anchor(), ANCHOR);
            assert_eq!(opened.game.rng, game.rng);
            assert_eq!(opened.game.state.decisions.pending.len(), 1);
            let p = &opened.game.state.decisions.pending[0];
            assert_eq!(
                (p.kind.as_str(), p.seat),
                (KIND, SeatId::new(actor, Role::Logistics))
            );
            assert!(
                opened
                    .events
                    .iter()
                    .all(|e| e.audience == Audience::Seat(p.seat))
            );
            let retried = evaluate(
                &rules,
                content(),
                &checkpoint(&opened.game),
                &Command::Advance,
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(&retried.game).unwrap(),
                serde_json::to_value(&opened.game).unwrap()
            );
            assert!(retried.events.is_empty());
            let cmd = respond(p, json!([]));
            let accepted = evaluate(&rules, content(), &opened.game, &cmd).unwrap();
            assert_eq!(accepted.game.rng, opened.game.rng);
            assert_eq!(
                serde_json::to_value(&accepted.game.state.logistics.truck_pools).unwrap(),
                serde_json::to_value(&opened.game.state.logistics.truck_pools).unwrap()
            );
            assert!(
                accepted
                    .events
                    .iter()
                    .all(|e| e.audience == Audience::Seat(p.seat))
            );
            assert!(evaluate(&rules, content(), &accepted.game, &cmd).is_err());
            let mut restored = checkpoint(&accepted.game);
            let mut state = accepted.game.state;
            let mut rng = CampaignRng::from_state(&accepted.game.rng);
            let mut restored_rng = CampaignRng::from_state(&restored.rng);
            let mut a = vec![];
            let mut b = vec![];
            dispatched_finish(rules, &mut state, &mut rng, &mut a).unwrap();
            dispatched_finish(rules, &mut restored.state, &mut restored_rng, &mut b).unwrap();
            assert_eq!(
                serde_json::to_value(&state).unwrap(),
                serde_json::to_value(&restored.state).unwrap()
            );
            assert_eq!(a, b);
            assert_eq!(rng.state(), restored_rng.state());
            assert!(state.logistics.truck_convoy.resolved);
            assert_eq!(state.cursor.anchor(), ANCHOR);
            assert!(state.decisions.pending.is_empty());
        }
    }
}

/// Bounded loaded-pool component fixture, explicitly not the required real setup witness.
fn loaded_fixture() -> (State, String) {
    let (mut s, id) = fixture();
    s.cursor.entered = true;
    for u in s.land.units.values_mut() {
        u.location = Location::Eliminated;
    }
    s.turn.weather = Some(crate::state::WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    let p = s
        .logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap();
    p.trucks = Trucks {
        light: 0,
        medium: 20,
        heavy: 0,
    };
    p.cargo = Supplies {
        ammo: 2,
        fuel: 200,
        stores: 2,
        water: 2,
    };
    p.activity_water = WaterPoints::new(20);
    pool_fuel::seed_created_pool(&mut s, &id).unwrap();
    (s, id)
}

/// Cases: airlog:49.18, airlog:52.42, airlog:53.25, land:21.41, land:3.6
#[test]
fn actual_dispatcher_move_buffers_only_then_pays_moves_and_recovers_once() {
    let (s, id) = loaded_fixture();
    let game: Game<Cna> = Game {
        state: s,
        rng: CampaignRng::from_seed([3; 32]).state(),
    };
    let rules = Cna::dev();
    let opened = evaluate(&rules, content(), &game, &Command::Advance).unwrap();
    let p = &opened.game.state.decisions.pending[0];
    let cmd = respond(p, json!([{"operation":"move","pool":id,"path":["C4021"]}]));
    let accepted = evaluate(&rules, content(), &opened.game, &cmd).unwrap();
    assert_eq!(accepted.game.rng, opened.game.rng);
    assert_eq!(
        serde_json::to_value(&accepted.game.state.logistics.truck_pools).unwrap(),
        serde_json::to_value(&opened.game.state.logistics.truck_pools).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&accepted.game.state.land.breakdown).unwrap(),
        serde_json::to_value(&opened.game.state.land.breakdown).unwrap()
    );
    assert!(
        accepted
            .events
            .iter()
            .all(|e| e.audience == Audience::Seat(p.seat))
    );
    let closed = evaluate(&rules, content(), &accepted.game, &Command::Advance).unwrap();
    let recovered = evaluate(
        &rules,
        content(),
        &checkpoint(&accepted.game),
        &Command::Advance,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&closed.game).unwrap(),
        serde_json::to_value(&recovered.game).unwrap()
    );
    assert_eq!(closed.events, recovered.events);
    let pool = closed
        .game
        .state
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id == id)
        .unwrap();
    assert_eq!(
        pool.location.as_ref().and_then(Location::hex),
        Some(&"C4021".into())
    );
    assert_eq!(pool.activity_water.get(), 0);
    assert!(pool.cargo.fuel < 200);
    assert_eq!(
        (pool.cargo.ammo, pool.cargo.stores, pool.cargo.water),
        (2, 2, 2)
    );
    assert_eq!(
        pool.trucks,
        Trucks {
            light: 0,
            medium: 20,
            heavy: 0
        }
    );
    assert!(closed.game.state.logistics.truck_convoy.resolved);
    let report = own_report(content(), &closed.game.state, Side::Axis, &id).unwrap();
    assert_eq!(report["timing"], "known");
    assert!(report["spent_cp_quarters"].as_i64().unwrap() > 0);
}

fn loss_wait_fixture() -> (Game<Cna>, String, String) {
    // Prior accumulated exposure is a bounded component fixture, not a setup proof.
    for seed in 0..16 {
        let (mut state, id) = loaded_fixture();
        state
            .land
            .breakdown
            .pools
            .entry(id.clone())
            .or_default()
            .accumulated_quarters = 400;
        let later = super::super::pools::add_truck_pool(
            &mut state.logistics,
            None,
            Side::Axis,
            Placement::Hex {
                hex: "C4020".into(),
            },
            Some(Location::Hex {
                hex: "C4020".into(),
            }),
            Trucks {
                light: 0,
                medium: 5,
                heavy: 0,
            },
            Supplies {
                fuel: 40,
                ..Supplies::default()
            },
        )
        .unwrap();
        state
            .logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.id == later)
            .unwrap()
            .activity_water = WaterPoints::new(5);
        pool_fuel::seed_created_pool(&mut state, &later).unwrap();
        let game = Game::<Cna> {
            state,
            rng: CampaignRng::from_seed([seed; 32]).state(),
        };
        let opened = evaluate(&Cna::dev(), content(), &game, &Command::Advance).unwrap();
        let cmd = respond(
            &opened.game.state.decisions.pending[0],
            json!([
                {"operation":"move","pool":id,"path":["C4021"]},
                {"operation":"move","pool":later,"path":["C4021"]}
            ]),
        );
        let accepted = evaluate(&Cna::dev(), content(), &opened.game, &cmd).unwrap();
        let stopped = evaluate(&Cna::dev(), content(), &accepted.game, &Command::Advance).unwrap();
        if stopped.game.state.land.breakdown.window.pool.is_some() {
            assert_eq!(stopped.game.state.cursor.anchor(), ANCHOR);
            assert_eq!(stopped.game.state.logistics.truck_convoy.next_order, 1);
            assert_eq!(
                stopped
                    .game
                    .state
                    .logistics
                    .truck_convoy
                    .waiting_pool
                    .as_deref(),
                Some(id.as_str())
            );
            assert_eq!(
                stopped
                    .game
                    .state
                    .logistics
                    .truck_pools
                    .iter()
                    .find(|p| p.id == later)
                    .unwrap()
                    .location
                    .as_ref()
                    .and_then(Location::hex),
                Some(&"C4020".into())
            );
            return (stopped.game, id, later);
        }
    }
    panic!("bounded independently seeded runs must exercise an actual positive breakdown roll");
}

fn accept_loss(game: &Game<Cna>) -> Game<Cna> {
    let pending = &game.state.decisions.pending[0];
    assert_eq!(pending.kind, breakdown::window::KIND);
    let request = Cna::dev().pending(content(), &game.state).remove(0);
    let mut local = CampaignRng::from_seed([9; 32]);
    let action = crate::baseline::random_breakdown(content(), &game.state, &request, &mut local);
    assert!(!action.is_null());
    let accepted = evaluate(&Cna::dev(), content(), game, &respond(pending, action)).unwrap();
    assert_eq!(accepted.game.rng, game.rng);
    assert_eq!(
        serde_json::to_value(&accepted.game.state.logistics).unwrap(),
        serde_json::to_value(&game.state.logistics).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&accepted.game.state.land.breakdown.markers).unwrap(),
        serde_json::to_value(&game.state.land.breakdown.markers).unwrap()
    );
    assert!(
        accepted
            .events
            .iter()
            .all(|e| e.audience == Audience::Seat(pending.seat))
    );
    accepted.game
}

/// Cases: land:21.35, land:21.41, land:21.43, airlog:53.12, land:3.6
#[test]
fn actual_dispatcher_loss_wait_checkpoint_conserves_and_continues_saved_order_once() {
    let (waiting, id, later) = loss_wait_fixture();
    let retry = evaluate(
        &Cna::dev(),
        content(),
        &checkpoint(&waiting),
        &Command::Advance,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&retry.game).unwrap(),
        serde_json::to_value(&waiting).unwrap()
    );
    assert!(retry.events.is_empty());
    let before = waiting
        .state
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id == id)
        .unwrap()
        .clone();
    let accepted = accept_loss(&waiting);
    let a = evaluate(&Cna::dev(), content(), &accepted, &Command::Advance).unwrap();
    let b = evaluate(
        &Cna::dev(),
        content(),
        &checkpoint(&accepted),
        &Command::Advance,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&a.game).unwrap(),
        serde_json::to_value(&b.game).unwrap()
    );
    assert_eq!(a.events, b.events);
    assert!(a.game.state.logistics.truck_convoy.resolved);
    assert!(a.game.state.land.breakdown.window.pool.is_none());
    let working = a
        .game
        .state
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id == id)
        .unwrap();
    let markers: Vec<_> = a
        .game
        .state
        .land
        .breakdown
        .markers
        .values()
        .filter(|m| m.source_pool.as_deref() == Some(id.as_str()))
        .collect();
    assert!(!markers.is_empty());
    assert_eq!(
        working.trucks.medium
            + markers
                .iter()
                .flat_map(|m| &m.pool_assets)
                .map(|a| a.points)
                .sum::<i32>(),
        before.trucks.medium
    );
    let mut holdings = working.cargo;
    for m in &markers {
        let goods = m.cargo.totals().unwrap();
        holdings.ammo += goods.ammo;
        holdings.fuel += goods.fuel;
        holdings.stores += goods.stores;
        holdings.water += goods.water;
        assert!(m.assets.is_empty() && m.passengers.is_empty());
        assert_eq!(m.transport, Trucks::default());
        assert!(m.pool_assets.iter().all(|a| {
            m.pool_fuel_cohorts
                .iter()
                .any(|g| g.id == a.cohort && g.count == a.points)
        }));
    }
    assert_eq!(holdings, before.cargo);
    assert_eq!(
        working.tank_fuel.get() + markers.iter().map(|m| m.tank_fuel.get()).sum::<i32>(),
        before.tank_fuel.get()
    );
    assert_eq!(
        working.activity_water.get() + markers.iter().map(|m| m.activity_water.get()).sum::<i32>(),
        before.activity_water.get()
    );
    let later = a
        .game
        .state
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id == later)
        .unwrap();
    assert_eq!(
        later.location.as_ref().and_then(Location::hex),
        Some(&"C4021".into())
    );
    assert_eq!(later.activity_water.get(), 0);
    assert!(later.cargo.fuel < 40);
}

/// Cases: land:21.42, land:21.43, airlog:53.12
#[test]
fn actual_dispatcher_late_marker_failure_rolls_back_complete_state_rng_and_events() {
    let (waiting, _, _) = loss_wait_fixture();
    let mut accepted = accept_loss(&waiting);
    accepted
        .state
        .land
        .breakdown
        .next_marker
        .insert(Side::Axis, u64::MAX);
    let before = serde_json::to_value(&accepted.state).unwrap();
    let mut rng = CampaignRng::from_state(&accepted.rng);
    let dice = rng.state();
    let mut events = vec![EngineEvent::new(
        Audience::Side(Side::Axis),
        cna_protocol::GameEvent::Note {
            text: "earlier committed event".into(),
        },
    )];
    let log = events.clone();
    let error =
        dispatched_finish(Cna::dev(), &mut accepted.state, &mut rng, &mut events).unwrap_err();
    assert!(matches!(error, EngineError::Invariant { .. }));
    assert_eq!(serde_json::to_value(&accepted.state).unwrap(), before);
    assert_eq!(rng.state(), dice);
    assert_eq!(events, log);
}

/// Cases: land:3.6, land:10.29, airlog:53.12, airlog:52.42
#[test]
fn actual_dispatcher_hidden_control_is_not_a_respond_oracle_and_zero_edge_has_no_cost() {
    let (base, id) = loaded_fixture();
    let high = content()
        .map
        .neighbors(&"C4021".into())
        .iter()
        .filter(|h| h.id != "C4020".into())
        .find_map(|h| {
            let mut candidate = base.clone();
            for u in candidate
                .land
                .units
                .values_mut()
                .filter(|u| u.side == Side::Commonwealth)
            {
                u.location = Location::Hex { hex: h.id.clone() };
                u.cohesion_quarters = 0;
            }
            crate::land::zoc::controlled(
                content(),
                &candidate,
                Side::Commonwealth,
                &"C4021".into(),
                false,
            )
            .unwrap()
            .then_some(candidate)
        })
        .expect("component pair requires a mapped source-passable control edge");
    let mut low = high.clone();
    for u in low
        .land
        .units
        .values_mut()
        .filter(|u| u.side == Side::Commonwealth)
    {
        u.cohesion_quarters = -104;
    }
    assert!(
        !crate::land::zoc::controlled(content(), &low, Side::Commonwealth, &"C4021".into(), false)
            .unwrap()
    );
    assert!(
        crate::land::zoc::controlled(content(), &high, Side::Commonwealth, &"C4021".into(), false)
            .unwrap()
    );
    let rules = Cna::dev();
    let a = Game::<Cna> {
        state: low,
        rng: CampaignRng::from_seed([3; 32]).state(),
    };
    let b = Game::<Cna> {
        state: high,
        rng: a.rng.clone(),
    };
    crate::testkit::assert_action_indistinguishable(
        &rules,
        content(),
        &a,
        &b,
        &Command::Advance,
        Side::Axis,
    );
    let a = evaluate(&rules, content(), &a, &Command::Advance)
        .unwrap()
        .game;
    let b = evaluate(&rules, content(), &b, &Command::Advance)
        .unwrap()
        .game;
    let cmd = respond(
        &a.state.decisions.pending[0],
        json!([{"operation":"move","pool":id,"path":["C4021"]}]),
    );
    crate::testkit::assert_action_indistinguishable(&rules, content(), &a, &b, &cmd, Side::Axis);
    let accepted = evaluate(&rules, content(), &b, &cmd).unwrap().game;
    let before = serde_json::to_value(&accepted.state.logistics.truck_pools).unwrap();
    let histories = serde_json::to_value(&accepted.state.logistics.cargo_history).unwrap();
    let breakdown = serde_json::to_value(&accepted.state.land.breakdown).unwrap();
    let posture = accepted.state.land.movement.pool_on_road.clone();
    let mut state = accepted.state;
    let mut rng = CampaignRng::from_state(&accepted.rng);
    let mut events = vec![];
    dispatched_finish(rules, &mut state, &mut rng, &mut events).unwrap();
    assert!(state.logistics.truck_convoy.resolved);
    assert_eq!(
        serde_json::to_value(&state.logistics.truck_pools).unwrap(),
        before
    );
    assert_eq!(
        serde_json::to_value(&state.logistics.cargo_history).unwrap(),
        histories
    );
    assert_eq!(
        serde_json::to_value(&state.land.breakdown).unwrap(),
        breakdown
    );
    assert!(!state.logistics.pool_fuel_segments.contains_key(&id));
    assert_eq!(state.land.movement.pool_on_road, posture);
    assert_eq!(rng.state(), accepted.rng);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].audience, Audience::Side(Side::Axis));
    assert!(
        matches!(&events[0].event, cna_protocol::GameEvent::Note { text } if text == "Convoy stops before an edge it may not enter; the rest of this order is discarded.")
    );
}

/// Cases: land:21.35, land:21.43, land:3.6, airlog:53.12
#[test]
fn actual_dispatcher_held_pending_restore_preserves_anchor_index_and_all_observer_streams() {
    let (mut waiting, _, later) = loss_wait_fixture();
    let loss_pending = std::mem::take(&mut waiting.state.decisions.pending);
    let mut local = CampaignRng::from_state(&waiting.rng);
    let mut opening = vec![];
    // Use the actual Water Distribution opening, with its full private schemas,
    // to exercise the existing window's held-pending checkpoint representation.
    super::super::water::enter(
        content(),
        &mut waiting.state,
        &mut Cx {
            rng: &mut local,
            events: &mut opening,
        },
        false,
    )
    .unwrap();
    let held = std::mem::take(&mut waiting.state.decisions.pending);
    assert!(!held.is_empty());
    assert_eq!(local.state(), waiting.rng);
    waiting
        .state
        .land
        .breakdown
        .window
        .pool
        .as_mut()
        .unwrap()
        .held = held.clone();
    waiting.state.decisions.pending = loss_pending;
    let accepted = accept_loss(&waiting);
    let mut a = accepted.state.clone();
    let mut b = checkpoint(&accepted).state;
    let mut ra = CampaignRng::from_state(&accepted.rng);
    let mut rb = ra.clone();
    let mut ea = vec![];
    let mut eb = vec![];
    dispatched_finish(Cna::dev(), &mut a, &mut ra, &mut ea).unwrap();
    dispatched_finish(Cna::dev(), &mut b, &mut rb, &mut eb).unwrap();
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&b).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&a.decisions.pending).unwrap(),
        serde_json::to_value(&held).unwrap()
    );
    assert_eq!(a.cursor.anchor(), ANCHOR);
    assert_eq!(a.logistics.truck_convoy.next_order, 1);
    assert!(a.logistics.truck_convoy.waiting_pool.is_none());
    assert!(!a.logistics.truck_convoy.resolved);
    assert_eq!(
        a.logistics
            .truck_pools
            .iter()
            .find(|p| p.id == later)
            .unwrap()
            .location
            .as_ref()
            .and_then(Location::hex),
        Some(&"C4020".into())
    );
    for perspective in cna_core::visibility::Perspective::all() {
        let visible = |events: &Vec<EngineEvent>| {
            events
                .iter()
                .filter(|e| perspective.can_see(&e.audience))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(visible(&ea), visible(&eb));
        assert_eq!(
            Cna::dev().view(content(), &a, perspective),
            Cna::dev().view(content(), &b, perspective)
        );
        assert_eq!(
            Cna::dev().observe(content(), &a, perspective),
            Cna::dev().observe(content(), &b, perspective)
        );
    }
    assert_eq!(ra.state(), rb.state());
    let before = serde_json::to_value(&a).unwrap();
    let dice = ra.state();
    let mut events = vec![];
    dispatched_finish(Cna::dev(), &mut a, &mut ra, &mut events).unwrap();
    assert_eq!(serde_json::to_value(&a).unwrap(), before);
    assert_eq!(ra.state(), dice);
    assert!(events.is_empty());
}

/// Cases: land:3.6, land:21.35, land:21.43, airlog:53.12
#[test]
fn actual_dispatcher_loss_and_finish_preserve_all_foreign_streams_across_hidden_stock() {
    let (a, id, _) = loss_wait_fixture();
    let mut b = checkpoint(&a);
    cargo_history::retire_debit(
        &mut b.state.logistics,
        &CargoSite::Pool(id.clone()),
        Supplies {
            fuel: 1,
            ..Supplies::default()
        },
    )
    .unwrap();
    b.state
        .logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .cargo
        .fuel -= 1;
    let rules = Cna::dev();
    crate::testkit::assert_indistinguishable(
        &rules,
        content(),
        &a.state,
        &b.state,
        Side::Commonwealth,
    );
    let command = |game: &Game<Cna>| {
        let request = rules.pending(content(), &game.state).remove(0);
        let mut local = CampaignRng::from_seed([9; 32]);
        let action =
            crate::baseline::random_breakdown(content(), &game.state, &request, &mut local);
        assert!(!action.is_null());
        respond(&game.state.decisions.pending[0], action)
    };
    let ca = command(&a);
    let cb = command(&b);
    crate::testkit::assert_actions_indistinguishable(
        &rules,
        content(),
        (&a, &ca),
        (&b, &cb),
        Side::Commonwealth,
    );
    let a = evaluate(&rules, content(), &a, &ca).unwrap().game;
    let b = evaluate(&rules, content(), &b, &cb).unwrap().game;
    crate::testkit::assert_action_indistinguishable(
        &rules,
        content(),
        &a,
        &b,
        &Command::Advance,
        Side::Commonwealth,
    );
    assert_eq!(a.rng, b.rng);
}

fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn fixture() -> (State, String) {
    let mut state = State::new(content()).unwrap();
    state.cursor.block = Block::PlayerHalf;
    state.cursor.op_stage = Some(1);
    state.cursor.half = Some(Half::A);
    state.cursor.index = PLAYER_HALF
        .iter()
        .position(|step| step.anchor == ANCHOR)
        .unwrap();
    state.turn.player_a = Some(Side::Axis);
    let id = super::super::pools::add_truck_pool(
        &mut state.logistics,
        None,
        Side::Axis,
        Placement::Hex {
            hex: "C4020".into(),
        },
        Some(Location::Hex {
            hex: "C4020".into(),
        }),
        Trucks {
            light: 2,
            medium: 3,
            heavy: 0,
        },
        Supplies::default(),
    )
    .unwrap();
    (state, id)
}

/// Cases: airlog:53.25, land:3.6
#[test]
fn owner_inspect_reports_unknown_and_mixed_without_numeric_fallback_or_seed() {
    let (mut s, id) = fixture();
    let before = serde_json::to_value(&s).unwrap();
    let report = own_report(content(), &s, Side::Axis, &id).unwrap();
    assert_eq!(report["timing"], "unknown");
    assert!(report.get("spent_cp_quarters").is_none());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    pool_fuel::seed_created_pool(&mut s, &id).unwrap();
    // A legitimate mixed current history retains real canonical fuel identities.
    // Positive physical CP without any funding record is trusted corruption.
    pool_fuel::spend_pool_segment_fuel(content(), &mut s, &id, 0).unwrap();
    let entry = s
        .logistics
        .cargo_history
        .motion
        .entries
        .iter_mut()
        .find(|e| e.site == CargoSite::Pool(id.clone()))
        .unwrap();
    entry.cohorts[1].spent_cp_quarters = 4;
    let before = serde_json::to_value(&s).unwrap();
    let report = own_report(content(), &s, Side::Axis, &id).unwrap();
    assert_eq!(report["timing"], "mixed");
    assert!(report.get("spent_cp_quarters").is_none());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: airlog:53.25, land:3.6
#[test]
fn own_report_corruption_stays_invariant_and_foreign_missing_are_identical() {
    let (mut s, id) = fixture();
    pool_fuel::seed_created_pool(&mut s, &id).unwrap();
    s.logistics
        .cargo_history
        .motion
        .entries
        .iter_mut()
        .find(|e| e.site == CargoSite::Pool(id.clone()))
        .unwrap()
        .cohorts[0]
        .count += 1;
    let before = serde_json::to_value(&s).unwrap();
    assert!(matches!(
        own_report(content(), &s, Side::Axis, &id),
        Err(Rejection::Engine(EngineError::Invariant { .. }))
    ));
    assert_eq!(
        serde_json::to_value(own_report(content(), &s, Side::Commonwealth, &id).unwrap_err())
            .unwrap(),
        serde_json::to_value(own_report(content(), &s, Side::Commonwealth, "missing").unwrap_err())
            .unwrap()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: airlog:48.0, airlog:53.12, land:3.6
#[test]
fn explicit_empty_list_opens_once_buffers_only_and_checkpoint_closes_once() {
    for strict in [false, true] {
        let (mut s, _) = fixture();
        let mut rng = CampaignRng::from_seed([4; 32]);
        let mut events = vec![];
        finish(
            content(),
            &mut s,
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(s.decisions.pending.len(), 1);
        assert_eq!(s.decisions.pending[0].kind, KIND);
        let opened = serde_json::to_value(&s).unwrap();
        let log = serde_json::to_value(&events).unwrap();
        let dice = rng.state();
        finish(
            content(),
            &mut s,
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap(), opened);
        assert_eq!(serde_json::to_value(&events).unwrap(), log);
        let pending = s.decisions.pending.remove(0);
        let before = serde_json::to_value(&s).unwrap();
        answer(
            content(),
            &mut s,
            &pending,
            &json!([]),
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        let accepted = serde_json::to_value(&s).unwrap();
        let mut normalized = accepted.clone();
        normalized["logistics"]["truck_convoy"]["submitted"] = Value::Null;
        assert_eq!(normalized, before);
        assert_eq!(serde_json::to_value(&events).unwrap(), log);
        assert_eq!(rng.state(), dice);
        assert!(
            answer(
                content(),
                &mut s,
                &pending,
                &json!([]),
                strict,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events
                }
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), accepted);
        let mut recovered: State = serde_json::from_value(accepted).unwrap();
        let mut recovered_rng = rng.clone();
        let mut recovered_events = vec![];
        finish(
            content(),
            &mut s,
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        finish(
            content(),
            &mut recovered,
            strict,
            &mut Cx {
                rng: &mut recovered_rng,
                events: &mut recovered_events,
            },
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::to_value(&recovered).unwrap()
        );
        assert!(s.logistics.truck_convoy.resolved);
        assert_eq!(s.cursor.anchor(), ANCHOR);
        assert_eq!(rng.state(), dice);
        let closed = serde_json::to_value(&s).unwrap();
        finish(
            content(),
            &mut s,
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap(), closed);
    }
}

/// Cases: airlog:53.12
#[test]
fn serialized_progress_rejects_invalid_keys_duplicate_pools_and_detached_wait() {
    let (s, id) = fixture();
    let key = key(&s).unwrap();
    let valid = json!({"key":key,"submitted":[{"operation":"move","pool":id,"path":[]}],
        "next_order":1,"waiting_pool":id,"resolved":false});
    assert!(serde_json::from_value::<TruckConvoyProcedure>(valid.clone()).is_ok());
    for change in ["duplicate", "index", "wait", "resolved", "key"] {
        let mut bad = valid.clone();
        match change {
            "duplicate" => {
                let row = bad["submitted"][0].clone();
                bad["submitted"].as_array_mut().unwrap().push(row);
            }
            "index" => bad["next_order"] = json!(2),
            "wait" => bad["waiting_pool"] = json!("different"),
            "resolved" => bad["resolved"] = json!(true),
            _ => bad["key"] = Value::Null,
        }
        assert!(
            serde_json::from_value::<TruckConvoyProcedure>(bad).is_err(),
            "{change}"
        );
    }
}
