use super::*;
use crate::Cna;
use crate::logistics::SupplySource;
use crate::seq::{Block, PLAYER_HALF};
use cna_content::scenario::Placement;
use cna_core::decision::{ActionSchema, DecisionResponse};
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
    loaded_fixture_with(Trucks {
        medium: 20,
        ..Trucks::default()
    })
}

fn loaded_fixture_with(trucks: Trucks) -> (State, String) {
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
    p.trucks = trucks;
    p.cargo = Supplies {
        ammo: 2,
        fuel: if trucks.light > 0 { 100 } else { 200 },
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
    loss_wait_fixture_with(Trucks {
        medium: 20,
        ..Trucks::default()
    })
}

fn loss_wait_fixture_with(trucks: Trucks) -> (Game<Cna>, String, String) {
    // Prior accumulated exposure is a bounded component fixture, not a setup proof.
    for seed in 0..16 {
        let (mut state, id) = loaded_fixture_with(trucks);
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

/// A real control primitive distinguishes hidden strength at departure, while
/// the destination remains legal in both worlds. This is a component fixture.
fn departure_control_fixture() -> (State, State, String) {
    let (base, id) = loaded_fixture();
    let high = content()
        .map
        .neighbors(&"C4020".into())
        .iter()
        .filter(|h| h.id != "C4021".into())
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
            let controls = |hex: &str| {
                crate::land::zoc::controlled(
                    content(),
                    &candidate,
                    Side::Commonwealth,
                    &hex.into(),
                    false,
                )
                .unwrap()
            };
            (controls("C4020") && !controls("C4021")).then_some(candidate)
        })
        .expect("source-passable departure control must leave the first destination legal");
    let mut low = high.clone();
    for u in low
        .land
        .units
        .values_mut()
        .filter(|u| u.side == Side::Commonwealth)
    {
        u.cohesion_quarters = -104;
    }
    assert_eq!(
        convoy_move::pool_departure_cp(content(), &high, &id, false).unwrap(),
        8
    );
    assert_eq!(
        convoy_move::pool_departure_cp(content(), &low, &id, false).unwrap(),
        0
    );
    assert!(matches!(
        convoy_move::adjudicate_pool_edge(
            content(),
            &high,
            &id,
            &"C4020".into(),
            &"C4021".into(),
            false
        )
        .unwrap(),
        convoy_move::PoolEdge::Pass(_)
    ));
    (low, high, id)
}

fn accepted_orders(state: State, orders: Value) -> Game<Cna> {
    let game = Game::<Cna> {
        state,
        rng: CampaignRng::from_seed([3; 32]).state(),
    };
    let opened = evaluate(&Cna::dev(), content(), &game, &Command::Advance).unwrap();
    let command = respond(&opened.game.state.decisions.pending[0], orders);
    evaluate(&Cna::dev(), content(), &opened.game, &command)
        .unwrap()
        .game
}

/// Cases: land:10.23, land:10.29, land:8.65, airlog:53.12, airlog:52.42
#[test]
fn actual_dispatcher_blocked_suffix_retains_exact_prefix_contact_and_later_order() {
    let (_, mut state, id) = departure_control_fixture();
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
    let cost = convoy_move::prepare_pool_route(
        content(),
        &state,
        Side::Axis,
        &id,
        &["C4021".into()],
        false,
    )
    .unwrap()
    .costs[0]
        .cp_quarters;
    let prefix = accepted_orders(
        state.clone(),
        json!([
            {"operation":"move","pool":id,"path":["C4021"]},
            {"operation":"move","pool":later,"path":["C4021"]}
        ]),
    );
    let full = accepted_orders(
        state,
        json!([
            {"operation":"move","pool":id,"path":["C4021","C4020"]},
            {"operation":"move","pool":later,"path":["C4021"]}
        ]),
    );
    let mut a = prefix.state;
    let mut b = checkpoint(&full).state;
    let mut ra = CampaignRng::from_state(&prefix.rng);
    let mut rb = CampaignRng::from_state(&full.rng);
    let mut ea = vec![];
    let mut eb = vec![];
    dispatched_finish(Cna::dev(), &mut a, &mut ra, &mut ea).unwrap();
    dispatched_finish(Cna::dev(), &mut b, &mut rb, &mut eb).unwrap();
    assert!(a.logistics.truck_convoy.resolved && b.logistics.truck_convoy.resolved);
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&b).unwrap()
    );
    assert_eq!(ra.state(), rb.state());
    for pool_id in [&id, &later] {
        let p = b
            .logistics
            .truck_pools
            .iter()
            .find(|p| &p.id == pool_id)
            .unwrap();
        assert_eq!(
            p.location.as_ref().and_then(Location::hex),
            Some(&"C4021".into())
        );
        assert_eq!(p.activity_water.get(), 0);
    }
    assert_eq!(b.logistics.pool_fuel_segments[&id].cp_quarters, cost + 8);
    assert_eq!(
        own_report(content(), &b, Side::Axis, &id).unwrap()["spent_cp_quarters"],
        cost + 8
    );
    let stop_notes: Vec<_> = eb.iter().filter(|e| matches!(
        &e.event, cna_protocol::GameEvent::Note { text }
            if text == "Convoy stops before an edge it may not enter; the rest of this order is discarded."
    )).collect();
    assert_eq!(stop_notes.len(), 1);
    assert_eq!(stop_notes[0].audience, Audience::Side(Side::Axis));
    let without_stop: Vec<_> = eb.into_iter().filter(|e| !matches!(
        &e.event, cna_protocol::GameEvent::Note { text }
            if text == "Convoy stops before an edge it may not enter; the rest of this order is discarded."
    )).collect();
    assert_eq!(ea, without_stop);
}

/// Cases: land:3.6, land:8.65, airlog:49.18, airlog:52.42, airlog:53.12
#[test]
fn actual_dispatcher_hidden_contact_fuel_stop_has_no_first_edge_effects() {
    let (mut low, mut high, id) = departure_control_fixture();
    let cost =
        convoy_move::prepare_pool_route(content(), &low, Side::Axis, &id, &["C4021".into()], false)
            .unwrap()
            .costs[0]
            .cp_quarters;
    let fuel = pool_fuel::plan_pool_segment_fuel(content(), &low, &id, cost)
        .unwrap()
        .funding
        .iter()
        .map(|p| p.increment.get())
        .sum::<i32>();
    let more = pool_fuel::plan_pool_segment_fuel(content(), &high, &id, cost + 8)
        .unwrap()
        .funding
        .iter()
        .map(|p| p.increment.get())
        .sum::<i32>();
    let fuel = (fuel + 9) / 10;
    assert!(more > fuel * 10);
    for state in [&mut low, &mut high] {
        state
            .logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap()
            .cargo
            .fuel = fuel;
    }
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
    let command = respond(
        &a.state.decisions.pending[0],
        json!([{"operation":"move","pool":id,"path":["C4021"]}]),
    );
    crate::testkit::assert_action_indistinguishable(
        &rules,
        content(),
        &a,
        &b,
        &command,
        Side::Axis,
    );
    let accepted = evaluate(&rules, content(), &b, &command).unwrap().game;
    let mut state = checkpoint(&accepted).state;
    let before = serde_json::to_value(&state).unwrap();
    let mut rng = CampaignRng::from_state(&accepted.rng);
    let mut events = vec![];
    dispatched_finish(rules, &mut state, &mut rng, &mut events).unwrap();
    let after = serde_json::to_value(&state).unwrap();
    assert_eq!(
        after["logistics"]["truck_pools"],
        before["logistics"]["truck_pools"]
    );
    assert_eq!(
        after["logistics"]["cargo_history"],
        before["logistics"]["cargo_history"]
    );
    assert_eq!(
        after["logistics"]["pool_fuel_segments"],
        before["logistics"]["pool_fuel_segments"]
    );
    assert_eq!(
        after["logistics"]["pool_fuel_accounts"],
        before["logistics"]["pool_fuel_accounts"]
    );
    assert_eq!(after["land"]["breakdown"], before["land"]["breakdown"]);
    assert_eq!(after["land"]["movement"], before["land"]["movement"]);
    assert_eq!(rng.state(), accepted.rng);
    assert!(state.logistics.truck_convoy.resolved);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].audience, Audience::Side(Side::Axis));
    assert!(
        matches!(&events[0].event, cna_protocol::GameEvent::Note { text }
        if text == "Convoy stops before unfunded travel.")
    );
}
/// Cases: land:3.6, land:8.65, airlog:53.25, airlog:52.42
#[test]
fn actual_dispatcher_hidden_contact_respects_physical_and_retained_cargo_ceilings() {
    for physical_ceiling in [true, false] {
        let (mut low, mut high, id) = departure_control_fixture();
        let cpa = ceiling(content(), pool(&low, Side::Axis, &id).unwrap().trucks).unwrap();
        let cost = convoy_move::prepare_pool_route(
            content(),
            &low,
            Side::Axis,
            &id,
            &["C4021".into()],
            false,
        )
        .unwrap()
        .costs[0]
            .cp_quarters;
        let spent = if physical_ceiling { cpa - cost } else { 1 };
        assert!(spent > 0);
        for state in [&mut low, &mut high] {
            // Canonical prior charge and physical timing, not unpaid positive CP.
            pool_fuel::spend_pool_segment_fuel(content(), state, &id, spent).unwrap();
            let groups: Vec<_> = pool_fuel::pool_segment_fuel_cohorts(state, &id)
                .unwrap()
                .iter()
                .map(motion::PhysicalTrucks::from)
                .collect();
            cargo_history::advance(
                state,
                Side::Axis,
                &CargoSite::Pool(id.clone()),
                cargo_history::CarrierTiming {
                    spent_cp_quarters: 0,
                    cpa_quarters: cpa,
                },
                spent,
            )
            .unwrap();
            motion::advance(
                state,
                Side::Axis,
                &CargoSite::Pool(id.clone()),
                &groups,
                spent,
            )
            .unwrap();
            if !physical_ceiling {
                // Bounded retained parcel fixture: its original carrier's lower
                // ceiling is valid and does not become the current truck CPA.
                let history = state
                    .logistics
                    .cargo_history
                    .histories
                    .get_mut(&CargoSite::Pool(id.clone()))
                    .unwrap();
                assert_eq!(history.lots.len(), 1);
                history.lots[0].ceiling_cp_quarters = spent + cost;
            }
        }
        let rules = Cna::dev();
        let a = Game::<Cna> {
            state: low,
            rng: CampaignRng::from_seed([3; 32]).state(),
        };
        let b = Game::<Cna> {
            state: high,
            rng: a.rng.clone(),
        };
        let a = evaluate(&rules, content(), &a, &Command::Advance)
            .unwrap()
            .game;
        let b = evaluate(&rules, content(), &b, &Command::Advance)
            .unwrap()
            .game;
        let command = respond(
            &a.state.decisions.pending[0],
            json!([{"operation":"move","pool":id,"path":["C4021"]}]),
        );
        crate::testkit::assert_action_indistinguishable(
            &rules,
            content(),
            &a,
            &b,
            &command,
            Side::Axis,
        );
        let accepted = evaluate(&rules, content(), &b, &command).unwrap().game;
        let mut state = checkpoint(&accepted).state;
        let before = serde_json::to_value(&state).unwrap();
        let mut rng = CampaignRng::from_state(&accepted.rng);
        let mut events = vec![];
        dispatched_finish(rules, &mut state, &mut rng, &mut events).unwrap();
        let mut after = serde_json::to_value(&state).unwrap();
        after["logistics"]["truck_convoy"] = before["logistics"]["truck_convoy"].clone();
        assert_eq!(after, before);
        assert_eq!(rng.state(), accepted.rng);
        assert!(state.logistics.truck_convoy.resolved);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].audience, Audience::Side(Side::Axis));
        let expected = if physical_ceiling {
            "Convoy stops before travel exceeding its current CPA."
        } else {
            "Convoy stops at the retained cargo CPA ceiling (53.25)."
        };
        assert!(
            matches!(&events[0].event, cna_protocol::GameEvent::Note { text } if text == expected)
        );
    }
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

/// Cases: scen:60.7, airlog:53.25, land:6.13, land:29.1
#[test]
fn actual_state_creation_and_weather_entry_seed_without_location_or_funding() {
    let created = State::new(content()).unwrap();
    assert!(!created.logistics.truck_pools.is_empty());
    assert!(created.logistics.pool_fuel_segments.is_empty());
    assert!(created.logistics.pool_fuel_accounts.is_empty());
    for pool in &created.logistics.truck_pools {
        let groups = pool_fuel::unfunded_pool_physical_cohorts(&created, &pool.id).unwrap();
        let timing = motion::query(
            &created,
            pool.side,
            &CargoSite::Pool(pool.id.clone()),
            &groups,
        )
        .unwrap();
        assert!(timing.iter().all(|g| g.spent_cp_quarters == 0));
        assert!(!created.land.movement.pool_on_road.contains(&pool.id));
    }
    for strict in [false, true] {
        let (mut state, id) = loaded_fixture();
        pool_fuel::spend_pool_segment_fuel(content(), &mut state, &id, 8).unwrap();
        let groups: Vec<_> = pool_fuel::pool_segment_fuel_cohorts(&state, &id)
            .unwrap()
            .iter()
            .map(motion::PhysicalTrucks::from)
            .collect();
        motion::advance(
            &mut state,
            Side::Axis,
            &CargoSite::Pool(id.clone()),
            &groups,
            8,
        )
        .unwrap();
        state.land.movement.pool_on_road.insert(id.clone());
        let accounts = serde_json::to_value(&state.logistics.pool_fuel_accounts).unwrap();
        let funding = serde_json::to_value(&state.logistics.pool_fuel_segments).unwrap();
        state.cursor.block = Block::OpStage;
        state.cursor.op_stage = Some(2);
        state.cursor.half = None;
        state.cursor.index = crate::seq::OPSTAGE
            .iter()
            .position(|step| step.anchor == "opstage.weather")
            .unwrap();
        state.cursor.entered = false;
        let mut expected = state.clone();
        let mut actual_rng = CampaignRng::from_seed([13; 32]);
        let mut expected_rng = actual_rng.clone();
        let mut actual_events = vec![];
        let mut expected_events = vec![];
        pool_fuel::seed_pool_opstage(&mut expected).unwrap();
        super::super::weather::determine(
            content(),
            &mut expected,
            &mut Cx {
                rng: &mut expected_rng,
                events: &mut expected_events,
            },
        )
        .unwrap();
        Cna { strict }
            .enter_step(
                content(),
                &mut state,
                &mut Cx {
                    rng: &mut actual_rng,
                    events: &mut actual_events,
                },
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        assert_eq!(actual_rng.state(), expected_rng.state());
        assert_eq!(actual_events, expected_events);
        assert_eq!(
            serde_json::to_value(&state.logistics.pool_fuel_accounts).unwrap(),
            accounts
        );
        assert_eq!(
            serde_json::to_value(&state.logistics.pool_fuel_segments).unwrap(),
            funding
        );
        assert!(state.land.movement.pool_on_road.contains(&id));
        assert!(
            motion::query(&state, Side::Axis, &CargoSite::Pool(id.clone()), &groups)
                .unwrap()
                .iter()
                .all(|g| g.spent_cp_quarters == 0)
        );
        assert_eq!(
            state
                .logistics
                .truck_pools
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .activity_water
                .get(),
            20
        );
    }
}

/// Cases: land:6.13, land:29.1, airlog:53.25
#[test]
fn actual_weather_entry_rejects_corrupt_prior_motion_before_dice_or_events() {
    for strict in [false, true] {
        let (mut state, id) = loaded_fixture();
        state.cursor.block = Block::OpStage;
        state.cursor.op_stage = Some(2);
        state.cursor.half = None;
        state.cursor.index = crate::seq::OPSTAGE
            .iter()
            .position(|step| step.anchor == "opstage.weather")
            .unwrap();
        state
            .logistics
            .cargo_history
            .motion
            .entries
            .iter_mut()
            .find(|e| e.site == CargoSite::Pool(id.clone()))
            .unwrap()
            .cohorts[0]
            .count += 1;
        let before = serde_json::to_value(&state).unwrap();
        let mut rng = CampaignRng::from_seed([13; 32]);
        let dice = rng.state();
        let mut events = vec![EngineEvent::new(
            Audience::Side(Side::Axis),
            cna_protocol::GameEvent::Note {
                text: "prior committed event".into(),
            },
        )];
        let log = events.clone();
        let error = Cna { strict }
            .enter_step(
                content(),
                &mut state,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events,
                },
            )
            .unwrap_err();
        assert!(matches!(error, EngineError::Invariant { .. }));
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        assert_eq!(rng.state(), dice);
        assert_eq!(events, log);
    }
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

fn first_setup(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => json!(options[0].id),
        ActionSchema::Unit { among } => json!(among[0]),
        ActionSchema::Integer { max, .. } => json!(max),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .map(|f| (f.name.clone(), first_setup(&f.schema)))
                .collect(),
        ),
        other => panic!("unexpected setup shape {other:?}"),
    }
}

fn graziani_response(p: &Pending, action: Value) -> Command {
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

/// Drive real authored setup and the scheduled stock-only water window. No reserve,
/// location, cargo or cursor is edited by this proof driver.
fn graziani_ready_convoy() -> (Game<Cna>, String, HexId, Vec<EngineEvent>) {
    let c = content();
    let rules = Cna::dev();
    let mut game: Game<Cna> = Game {
        state: State::new(c).unwrap(),
        rng: CampaignRng::from_seed([12; 32]).state(),
    };
    // Derive an actual source-authorized shared placement, including the
    // fixed-enemy four-hex exclusion and surveyed land. No geometry is injected.
    let source_pool = game
        .state
        .logistics
        .truck_pools
        .iter()
        .find(|p| {
            p.side == Side::Axis
                && p.trucks.light == 30
                && p.trucks.medium == 100
                && p.trucks.heavy == 25
        })
        .unwrap();
    let pool_domain = crate::setup::placement::choices_with_profile(
        c,
        &source_pool.placement,
        Side::Axis,
        "scen:59.44",
        false,
    )
    .unwrap();
    let crate::state::DumpLocation::AwaitingSetup { placement } =
        &game.state.logistics.dumps["ax_dump_1"].location
    else {
        panic!()
    };
    let dump_domain = crate::setup::placement::choices_with_profile(
        c,
        placement,
        Side::Axis,
        "scen:60.34",
        false,
    )
    .unwrap();
    let origin = dump_domain.iter().filter(|l| pool_domain.contains(l))
        .filter_map(Location::hex).find(|hex| c.map.neighbors(hex).iter().any(|h|
            matches!(c.map.terrain_survey(&h.id), cna_content::map::Survey::Present(t) if t != "sea")
            && crate::land::map::truck_step_cost(c, Side::Axis,
                &Trucks { light:30, medium:0, heavy:0 }, hex, &h.id, false, false).is_ok()
        )).expect("published regions provide an eligible known-land pool/dump placement").clone();
    let mut events = vec![];
    let mut selected = None;
    let mut loaded = false;
    let mut issued = false;
    for _ in 0..100_000 {
        if game.state.decisions.pending.is_empty() {
            let t = evaluate(&rules, c, &game, &Command::Advance).unwrap();
            game = t.game;
            events.extend(t.events);
            continue;
        }
        let p = game.state.decisions.pending[0].clone();
        if p.kind == KIND && p.seat.side == Side::Axis {
            assert!(game.state.setup.closed && loaded && issued);
            return (game, selected.unwrap(), origin, events);
        }
        let mut action = if game.state.setup.closed {
            let request = rules
                .pending(c, &game.state)
                .into_iter()
                .find(|r| r.id == p.id)
                .unwrap();
            let mut local = CampaignRng::from_seed([1; 32]);
            crate::baseline::logistics_orders_with_profile(
                c,
                &game.state,
                &request,
                &mut local,
                false,
            )
            .unwrap()
            .unwrap_or_else(|| {
                if p.space.pass.is_some() {
                    Value::Null
                } else {
                    first_setup(&p.space.schema)
                }
            })
        } else {
            first_setup(&p.space.schema)
        };
        if let Some(task) = game.state.setup.tasks.get(&p.id) {
            match task {
                crate::setup::SetupTask::Dump { dump, .. } if dump.starts_with("ax_dump_") => {
                    let ActionSchema::Choice { options } = &p.space.schema else {
                        panic!()
                    };
                    assert!(options.iter().any(|o| o.id == origin.to_string()));
                    action = json!(origin);
                }
                crate::setup::SetupTask::Pool { pool, .. }
                    if selected.is_none()
                        && game.state.logistics.truck_pools.iter().any(|q| {
                            q.id == *pool
                                && q.side == Side::Axis
                                && q.trucks.light == 30
                                && q.trucks.medium == 100
                        }) =>
                {
                    action = json!({"destination":origin,"light":30,"medium":0,"heavy":0});
                }
                crate::setup::SetupTask::Preload { asset, operation }
                    if selected
                        .as_ref()
                        .is_some_and(|id| asset.key() == format!("pool:{id}")) =>
                {
                    if operation == "menu" && !loaded {
                        action = json!("load");
                    } else if operation == "load" {
                        action = json!({"light":{"fuel":100}});
                        loaded = true;
                    }
                }
                _ => {}
            }
        }
        if p.kind == super::super::batches::WATER && p.seat.side == Side::Axis {
            let id = selected.as_ref().unwrap_or_else(|| {
                panic!(
                    "no selected pool after setup: {:?}",
                    game.state.logistics.truck_pools
                )
            });
            let need =
                super::super::water::pools::activity_need(c, &game.state, Side::Axis, id).unwrap();
            assert!(need == 30 || need == 60);
            let source =
                serde_json::to_string(&super::super::SupplySource::Dump("ax_dump_1".into()))
                    .unwrap();
            action = json!({"allocations":[],"wells":[],"pool_allocations":[
                {"pool":id,"activity":need,"draws":[{"source":source,"stores":0,"water":need}]}]});
            let before = serde_json::to_value(&game.state.logistics.truck_pools).unwrap();
            let stock = game.state.logistics.dumps["ax_dump_1"].supplies.water;
            let t = evaluate(&rules, c, &game, &graziani_response(&p, action)).unwrap();
            assert_eq!(t.game.rng, game.rng);
            assert_eq!(
                serde_json::to_value(&t.game.state.logistics.truck_pools).unwrap(),
                before
            );
            assert_eq!(
                t.game.state.logistics.dumps["ax_dump_1"].supplies.water,
                stock
            );
            game = t.game;
            events.extend(t.events);
            issued = true;
            continue;
        }
        let t = evaluate(&rules, c, &game, &graziani_response(&p, action.clone()))
            .unwrap_or_else(|e| panic!("{} {:?}: {e:?}", p.kind, action));
        game = t.game;
        events.extend(t.events);
        if p.kind == "cna.setup.preload" && action == json!({"light":{"fuel":100}}) {
            let id = selected.as_ref().unwrap();
            assert_eq!(
                game.state
                    .logistics
                    .truck_pools
                    .iter()
                    .find(|q| &q.id == id)
                    .unwrap()
                    .cargo
                    .fuel,
                100
            );
            assert!(!game.state.setup.closed);
        }
        if selected.is_none() {
            selected = game
                .state
                .logistics
                .truck_pools
                .iter()
                .find(|q| {
                    q.side == Side::Axis
                        && q.trucks
                            == Trucks {
                                light: 30,
                                medium: 0,
                                heavy: 0,
                            }
                        && game
                            .state
                            .setup
                            .pool_locations
                            .get(&q.id)
                            .and_then(Location::hex)
                            == Some(&origin)
                })
                .map(|q| q.id.clone());
        }
    }
    panic!("real Graziani proof did not reach convoy within campaign bound");
}

/// Cases: scen:59.44, scen:59.45, scen:60.34, airlog:52.42, airlog:53.25, land:21.41
#[test]
fn real_graziani_setup_stock_issue_move_and_checkpoint_use_actual_dispatcher() {
    let c = content();
    let (game, id, origin, _) = graziani_ready_convoy();
    let pool = game
        .state
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id == id)
        .unwrap();
    assert_eq!(
        pool.location.as_ref().and_then(Location::hex),
        Some(&origin)
    );
    assert_eq!(
        pool.trucks,
        Trucks {
            light: 30,
            medium: 0,
            heavy: 0
        }
    );
    // Real weekly storage loss precedes first-stage issue: Axis6%, normal weather.
    // Cases: airlog:49.3, airlog:52.44. The legal setup preload above remains100.
    assert_eq!(
        game.state.turn.weather.as_ref().unwrap().kind,
        WeatherKind::Normal
    );
    assert_eq!(pool.cargo.fuel, 100 - 100 * 6 / 100);
    assert_eq!(pool.activity_water.get(), 30);
    assert_eq!(
        game.state.logistics.dumps["ax_dump_1"].supplies.water,
        200 - 200 * 6 / 100 - 30
    );
    // Case: airlog:54.2. Light supply convoy40CP; Medium/Heavy30CP.
    assert_eq!(ceiling(c, pool.trucks).unwrap(), 160);
    let before_logistics = game.state.logistics.clone();
    let origin = pool.location.as_ref().unwrap().hex().unwrap();
    let target = c
        .map
        .neighbors(origin)
        .into_iter()
        .find_map(|h| {
            let path = vec![h.id.clone()];
            let plan = vec![TruckConvoyOrder::Move {
                pool: id.clone(),
                path: path.clone(),
            }];
            (validate_plan(c, &game.state, Side::Axis, &plan, false).is_ok()
                && matches!(
                    convoy_move::adjudicate_pool_edge(c, &game.state, &id, origin, &h.id, false),
                    Ok(convoy_move::PoolEdge::Pass(_))
                ))
            .then_some(h.id.clone())
        })
        .expect("authored setup has an eligible neighboring convoy edge");
    let cost = convoy_move::prepare_pool_route(
        c,
        &game.state,
        Side::Axis,
        &id,
        std::slice::from_ref(&target),
        false,
    )
    .unwrap()
    .costs
    .remove(0);
    let contact = convoy_move::pool_departure_cp(c, &game.state, &id, false).unwrap();
    let cp = cost.cp_quarters + contact;
    let exact_fuel = c
        .tables
        .airlog
        .fuel_consumption
        .fuel_for(1, (cp + 3) / 4)
        .unwrap()
        .get()
        * 30;
    let quote = pool_fuel::plan_pool_segment_fuel(c, &game.state, &id, cp).unwrap();
    assert_eq!(quote.ledger.paid_cost.get(), exact_fuel);
    assert!(exact_fuel > 0);
    let pending = game.state.decisions.pending[0].clone();
    let cmd = graziani_response(
        &pending,
        json!([{"operation":"move","pool":id,"path":[target]}]),
    );
    let accepted = evaluate(&Cna::dev(), c, &game, &cmd).unwrap();
    assert_eq!(
        serde_json::to_value(&accepted.game.state.logistics.truck_pools).unwrap(),
        serde_json::to_value(&game.state.logistics.truck_pools).unwrap()
    );
    assert_eq!(accepted.game.rng, game.rng);
    let restored: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&accepted.game).unwrap()).unwrap();
    let moved = evaluate(&Cna::dev(), c, &accepted.game, &Command::Advance).unwrap();
    let resumed = evaluate(&Cna::dev(), c, &restored, &Command::Advance).unwrap();
    assert_eq!(
        serde_json::to_value(&moved.game).unwrap(),
        serde_json::to_value(&resumed.game).unwrap()
    );
    assert_eq!(moved.events, resumed.events);
    let pool = moved
        .game
        .state
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id == id)
        .unwrap();
    assert_eq!(
        pool.location.as_ref().and_then(Location::hex),
        Some(&target)
    );
    assert_eq!(pool.activity_water.get(), 0);
    let ledger = &moved.game.state.logistics.pool_fuel_segments[&id];
    assert_eq!(ledger.cp_quarters, cp);
    assert_eq!(ledger.paid_cost.get(), exact_fuel);
    assert_eq!(ledger.draws, quote.ledger.draws);
    let mut source_debit = 0;
    for draw in &quote.draws {
        let points = draw.amount.fuel.ceil_points().get();
        source_debit += draw.amount.fuel.get();
        match &draw.source {
            SupplySource::Dump(source) => assert_eq!(
                moved.game.state.logistics.dumps[source].supplies.fuel,
                before_logistics.dumps[source].supplies.fuel - points
            ),
            SupplySource::PoolStock(source) => {
                let before = before_logistics
                    .truck_pools
                    .iter()
                    .find(|p| &p.id == source)
                    .unwrap();
                let after = moved
                    .game
                    .state
                    .logistics
                    .truck_pools
                    .iter()
                    .find(|p| &p.id == source)
                    .unwrap();
                assert_eq!(after.cargo.fuel, before.cargo.fuel - points);
            }
            other => panic!("real witness must debit actual friendly stock: {other:?}"),
        }
    }
    assert!(source_debit >= exact_fuel && source_debit - exact_fuel < 10);
    assert_eq!(
        own_report(c, &moved.game.state, Side::Axis, &id).unwrap()["spent_cp_quarters"],
        cp
    );
    assert_eq!(
        moved.game.state.land.movement.pool_on_road.contains(&id),
        cost.on_network
    );
    assert!(
        moved
            .game
            .state
            .logistics
            .pool_fuel_segments
            .contains_key(&id)
    );
}
/// Cases: land:3.6, land:29.1, airlog:53.25
#[test]
fn actual_dispatcher_source_and_history_errors_preserve_buffer_state_rng_and_prior_events() {
    for fault in 0..3 {
        let (s, id) = loaded_fixture();
        let opened = evaluate(
            &Cna::dev(),
            content(),
            &Game::<Cna> {
                state: s,
                rng: CampaignRng::from_seed([3; 32]).state(),
            },
            &Command::Advance,
        )
        .unwrap()
        .game;
        let command = respond(
            &opened.state.decisions.pending[0],
            json!([{"operation":"move","pool":id,"path":["C4021"]}]),
        );
        let accepted = evaluate(&Cna::dev(), content(), &opened, &command)
            .unwrap()
            .game;
        let corrupt = |state: &mut State| match fault {
            0 => state.turn.weather = None,
            1 => {
                state
                    .logistics
                    .cargo_history
                    .motion
                    .entries
                    .iter_mut()
                    .find(|e| e.site == CargoSite::Pool(id.clone()))
                    .unwrap()
                    .cohorts[0]
                    .count += 1
            }
            _ => {
                state.logistics.cargo_history.histories.insert(
                    CargoSite::Pool(id.clone()),
                    cargo_history::CargoHistory {
                        stage: super::super::water::WaterStage::current(state),
                        lots: vec![cargo_history::CargoLot {
                            id: "axis.cargo.1".into(),
                            goods: Supplies {
                                ammo: 1,
                                ..Supplies::default()
                            },
                            spent_cp_quarters: 4,
                            ceiling_cp_quarters: 120,
                            continuous_first_line: false,
                        }],
                    },
                );
            }
        };
        let mut rejected = opened.clone();
        corrupt(&mut rejected.state);
        let before = serde_json::to_value(&rejected).unwrap();
        let Rejection::Engine(expected) =
            evaluate(&Cna::dev(), content(), &rejected, &command).unwrap_err()
        else {
            panic!("must preserve a typed engine error")
        };
        match (fault, &expected) {
            (0, EngineError::Unsupported { case, .. }) => assert_eq!(case, "land:29.1"),
            (1, EngineError::Invariant { .. }) => {}
            (2, EngineError::Unsupported { case, .. }) => assert_eq!(case, "airlog:53.25"),
            _ => panic!("wrong error {expected:?}"),
        }
        assert_eq!(serde_json::to_value(&rejected).unwrap(), before);
        let mut state = accepted.state;
        corrupt(&mut state);
        let before = serde_json::to_value(&state).unwrap();
        let mut rng = CampaignRng::from_state(&accepted.rng);
        let dice = rng.state();
        let mut events = vec![EngineEvent::new(
            Audience::Side(Side::Axis),
            cna_protocol::GameEvent::Note {
                text: "prior committed event".into(),
            },
        )];
        let prior = events.clone();
        assert_eq!(
            dispatched_finish(Cna::dev(), &mut state, &mut rng, &mut events).unwrap_err(),
            expected
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        assert_eq!(rng.state(), dice);
        assert_eq!(events, prior);
        // A checkpoint retry returns the same source error without a new outcome.
        let mut restored: State = serde_json::from_value(before.clone()).unwrap();
        assert_eq!(
            dispatched_finish(Cna::dev(), &mut restored, &mut rng, &mut events).unwrap_err(),
            expected
        );
        assert_eq!(serde_json::to_value(&restored).unwrap(), before);
        assert_eq!(rng.state(), dice);
        assert_eq!(events, prior);
    }
}

/// Cases: land:21.35, land:21.41, land:21.43, airlog:53.12, land:3.6
#[test]
fn actual_dispatcher_light_and_heavy_loss_preserve_exact_type_lineage_and_all_holdings() {
    for trucks in [
        Trucks {
            light: 20,
            ..Trucks::default()
        },
        Trucks {
            heavy: 20,
            ..Trucks::default()
        },
    ] {
        let (waiting, id, _) = loss_wait_fixture_with(trucks);
        let before = waiting
            .state
            .logistics
            .truck_pools
            .iter()
            .find(|p| p.id == id)
            .unwrap();
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
        let working = a
            .game
            .state
            .logistics
            .truck_pools
            .iter()
            .find(|p| p.id == id)
            .unwrap();
        let mut counts = working.trucks;
        let mut goods = working.cargo;
        let mut fuel = working.tank_fuel.get();
        let mut water = working.activity_water.get();
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
        for m in markers {
            for asset in &m.pool_assets {
                let cohort = m
                    .pool_fuel_cohorts
                    .iter()
                    .find(|g| g.id == asset.cohort)
                    .unwrap();
                assert_eq!(cohort.count, asset.points);
                match cohort.kind {
                    segment::FuelTruckKind::Light => counts.light += cohort.count,
                    segment::FuelTruckKind::Medium => counts.medium += cohort.count,
                    segment::FuelTruckKind::Heavy => counts.heavy += cohort.count,
                }
            }
            let held = m.cargo.totals().unwrap();
            goods.ammo += held.ammo;
            goods.fuel += held.fuel;
            goods.stores += held.stores;
            goods.water += held.water;
            fuel += m.tank_fuel.get();
            water += m.activity_water.get();
            assert!(m.assets.is_empty() && m.passengers.is_empty());
            assert_eq!(m.transport, Trucks::default());
        }
        assert_eq!(counts, before.trucks);
        assert_eq!(goods, before.cargo);
        assert_eq!(fuel, before.tank_fuel.get());
        assert_eq!(water, before.activity_water.get());
        assert!(a.game.state.logistics.truck_convoy.resolved);
        assert!(a.game.state.land.breakdown.window.pool.is_none());
    }
}
