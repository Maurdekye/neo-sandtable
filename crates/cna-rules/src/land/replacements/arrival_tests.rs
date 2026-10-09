//! Kernel fixtures only. Whole-window/engine proofs follow the retained-limit caller.
use super::*;
use super::scheduling::{key, side, side_mut};
use cna_core::{engine::Rejection, ids::SeatId};
use cna_protocol::Role;

fn fixture() -> (crate::CnaContent, crate::State, UnitId, Location, Location) {
    let content = crate::CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = crate::State::new(&content).unwrap();
    state.cursor.block = crate::seq::Block::OpStage;
    state.cursor.index = crate::seq::OPSTAGE.iter().position(|step| step.anchor == arrival::ANCHOR).unwrap();
    state.cursor.game_turn = 4;
    state.cursor.op_stage = Some(1);
    state.cursor.entered = true;
    let current = key(&state);
    state.land.replacements.sides = Side::ALL.into_iter().map(|owner| {
        let mut own = SideReplacements::new(owner);
        own.arrival = ReplacementProcedure { key: Some(current.clone()), phase: ProcedurePhase::Awaiting { submitted: None } };
        own
    }).collect();
    let cities = arrival::cities(&content, false).unwrap();
    let cairo = cities.iter().find(|(name, _)| name == "cairo").unwrap().1.clone();
    let alexandria = cities.iter().find(|(name, _)| name == "alexandria").unwrap().1.clone();
    let recipient = state.land.units.keys().find(|id| {
        let source = &content.units.units[*id];
        source.side == Side::Commonwealth && source.infantry_kind.is_some()
            && source.class.as_ref().and_then(|class| content.units.classes.get(class))
                .is_some_and(|class| class.unit_type == "infantry")
    }).unwrap().clone();
    // A reduced arrival-kernel fixture places a verified real own recipient explicitly.
    // It does not establish a new Graziani production schedule or training entitlement.
    state.land.units.get_mut(&recipient).unwrap().location = cairo.clone();
    (content, state, recipient, cairo, alexandria)
}

fn truck_order(state: &mut crate::State, points: i32) -> String {
    let own = side_mut(state, Side::Commonwealth).unwrap();
    let id = own.allocate_id().unwrap();
    own.orders.push(ScheduledReplacement { id: id.clone(),
        row: ProductionRowRef::CommonwealthTruck { column: "gt_1_to_30".into(), kind: FuelTruckKind::Medium },
        kind: ProducedKind::Trucks { kind: FuelTruckKind::Medium }, points,
        due: OpStageKey { game_turn: 4, op_stage: 1 }, lane: None });
    id
}

fn foot_order(state: &mut crate::State) -> String {
    let own = side_mut(state, Side::Commonwealth).unwrap();
    let id = own.allocate_id().unwrap();
    own.orders.push(ScheduledReplacement { id: id.clone(),
        row: ProductionRowRef::CommonwealthInfantry { column: "gt_3_to_30".into() },
        kind: ProducedKind::Points { kind: ReplacementKind::Infantry { nation: ReplacementNation::Commonwealth } }, points: 1,
        due: OpStageKey { game_turn: 4, op_stage: 1 }, lane: None });
    id
}

fn quota() -> CWArrivalLimit {
    CWArrivalLimit { game_turn: 4, total_trucks: 8, alexandria_trucks: 0, infantry_city: None }
}

fn accept(state: &mut crate::State, plan: &ArrivalPlan) {
    side_mut(state, Side::Commonwealth).unwrap().arrival.phase = ProcedurePhase::Awaiting {
        submitted: Some(ReplacementPlan::Arrival(plan.clone())),
    };
}

fn open_window(content: &crate::CnaContent, state: &mut crate::State) {
    state.decisions.pending.clear();
    for own in &mut state.land.replacements.sides { own.arrival = ReplacementProcedure::default(); }
    let mut rng = cna_core::dice::CampaignRng::from_seed([41; 32]);
    let mut events = vec![];
    arrival::enter_arrival(content, state, false, &mut cna_core::engine::Cx { rng: &mut rng, events: &mut events }).unwrap();
    assert_eq!(state.decisions.pending.len(), 2);
    assert!(state.decisions.pending.iter().all(|pending| pending.kind == arrival::ARRIVAL && pending.seat.role == Role::Logistics && pending.space.pass.is_some()));
}

fn command(state: &crate::State, owner: Side, action: serde_json::Value) -> cna_core::engine::Command {
    let pending = state.decisions.pending.iter().find(|pending| pending.kind == arrival::ARRIVAL && pending.seat.side == owner).unwrap();
    cna_core::engine::Command::Respond(cna_core::decision::DecisionResponse {
        decision_id: pending.id.clone(), seat: pending.seat, controller_epoch: 0,
        decision_revision: pending.revision, idempotency_key: format!("{}-initial", pending.id), action, public_explanation: None,
    })
}

/// Cases: land:20.41, land:20.78A, land:20.78B, land:3.6
#[test]
fn real_respond_buffers_without_quota_stock_or_counter_changes_and_hides_own_orders() {
    use cna_core::engine::{Game, evaluate};
    let (content, mut state, _, cairo, _) = fixture();
    let id = truck_order(&mut state, 1);
    side_mut(&mut state, Side::Commonwealth).unwrap().cw_arrival_limits.push(quota());
    open_window(&content, &mut state);
    let locations = arrival::cities(&content, false).unwrap();
    let location = locations.iter().position(|(_, loc)| loc == &cairo).unwrap();
    let game = Game::<crate::Cna> { state, rng: cna_core::dice::CampaignRng::from_seed([42; 32]).state() };
    let cmd = command(&game.state, Side::Commonwealth, serde_json::json!({"choices":[{"order":id,"location":location}]}));
    let logistics = serde_json::to_vec(&game.state.logistics).unwrap();
    let owned = side(&game.state, Side::Commonwealth).unwrap();
    let orders = serde_json::to_vec(&owned.orders).unwrap();
    let limits = serde_json::to_vec(&owned.cw_arrival_limits).unwrap();
    let transition = evaluate(&crate::Cna::dev(), &content, &game, &cmd).unwrap();
    let after = side(&transition.game.state, Side::Commonwealth).unwrap();
    assert_eq!(serde_json::to_vec(&after.orders).unwrap(), orders);
    assert_eq!(serde_json::to_vec(&after.cw_arrival_limits).unwrap(), limits);
    assert_eq!(serde_json::to_vec(&transition.game.state.logistics).unwrap(), logistics);
    assert!(after.counters.is_empty());
    assert_eq!(transition.game.rng, game.rng);
    assert!(matches!(after.arrival.phase, ProcedurePhase::Awaiting { submitted: Some(ReplacementPlan::Arrival(_)) }));
    let mut hidden = game.clone();
    side_mut(&mut hidden.state, Side::Commonwealth).unwrap().orders[0].points = 2;
    crate::testkit::assert_action_indistinguishable(&crate::Cna::dev(), &content, &game, &hidden, &cmd, Side::Axis);
    let mut wrong = cmd.clone();
    let cna_core::engine::Command::Respond(response) = &mut wrong else { unreachable!() };
    response.seat.role = Role::FrontLine;
    assert!(evaluate(&crate::Cna::dev(), &content, &game, &wrong).is_err());
}

/// Cases: land:20.41, land:20.78A, land:20.78B, land:3.6
#[test]
fn zero_due_uses_same_public_window_and_missing_legacy_limit_never_becomes_zero() {
    use cna_core::engine::{Game, evaluate};
    let (content, mut empty, _, cairo, _) = fixture();
    let mut due_state = empty.clone();
    truck_order(&mut due_state, 1);
    open_window(&content, &mut empty);
    open_window(&content, &mut due_state);
    let observer_schedule = |state: &crate::State| state.decisions.pending.iter().map(|pending| (pending.seat, pending.kind.clone(), pending.summary.clone())).collect::<Vec<_>>();
    assert_eq!(observer_schedule(&empty), observer_schedule(&due_state));
    let empty = Game::<crate::Cna> { state: empty, rng: cna_core::dice::CampaignRng::from_seed([43; 32]).state() };
    let pass = evaluate(&crate::Cna::dev(), &content, &empty, &command(&empty.state, Side::Commonwealth, serde_json::Value::Null)).unwrap();
    assert!(side(&pass.game.state, Side::Commonwealth).unwrap().cw_arrival_limits.is_empty());
    assert_eq!(pass.game.rng, empty.rng);
    let id = side(&due_state, Side::Commonwealth).unwrap().orders[0].id.clone();
    let location = arrival::cities(&content, false).unwrap().iter().position(|(_, loc)| loc == &cairo).unwrap();
    let due_game = Game::<crate::Cna> { state: due_state, rng: empty.rng.clone() };
    let before = serde_json::to_vec(&due_game).unwrap();
    let result = evaluate(&crate::Cna::dev(), &content, &due_game, &command(&due_game.state, Side::Commonwealth, serde_json::json!({"choices":[{"order":id,"location":location}]})));
    assert!(matches!(result, Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:20.78A"));
    assert_eq!(serde_json::to_vec(&due_game).unwrap(), before);
}

/// Cases: land:20.41, land:20.78A
#[test]
fn real_advance_late_pool_failure_preserves_whole_game_rng_and_accepted_buffers() {
    use cna_core::engine::{Command, Game, evaluate};
    let (content, mut state, recipient, cairo, _) = fixture();
    let foot = foot_order(&mut state);
    let truck = truck_order(&mut state, 1);
    state.logistics.truck_pool_serial.insert(Side::Commonwealth, u64::MAX);
    side_mut(&mut state, Side::Commonwealth).unwrap().cw_arrival_limits.push(quota());
    open_window(&content, &mut state);
    let location = arrival::cities(&content, false).unwrap().iter().position(|(_, loc)| loc == &cairo).unwrap();
    let mut game = Game::<crate::Cna> { state, rng: cna_core::dice::CampaignRng::from_seed([44; 32]).state() };
    let own = command(&game.state, Side::Commonwealth, serde_json::json!({"choices":[{"order":foot,"recipient":recipient,"location":location},{"order":truck,"location":location}]}));
    game = evaluate(&crate::Cna::dev(), &content, &game, &own).unwrap().game;
    let pass = command(&game.state, Side::Axis, serde_json::Value::Null);
    game = evaluate(&crate::Cna::dev(), &content, &game, &pass).unwrap().game;
    let restored: Game<crate::Cna> = serde_json::from_slice(&serde_json::to_vec(&game).unwrap()).unwrap();
    assert!(restored.state.decisions.pending.is_empty());
    let before = serde_json::to_vec(&restored).unwrap();
    assert!(matches!(evaluate(&crate::Cna::dev(), &content, &restored, &Command::Advance), Err(Rejection::Engine(EngineError::Invariant { detail })) if detail.contains("identity space exhausted")));
    assert_eq!(serde_json::to_vec(&restored).unwrap(), before);
    assert!(matches!(side(&restored.state, Side::Commonwealth).unwrap().arrival.phase, ProcedurePhase::Awaiting { submitted: Some(ReplacementPlan::Arrival(_)) }));
}

/// Cases: land:20.41, land:20.43, land:20.78A, land:20.78B
#[test]
fn initial_logistics_assignment_is_pure_and_finish_creates_unabsorbed_uncredited_points() {
    let (content, mut state, recipient, cairo, _) = fixture();
    let id = foot_order(&mut state);
    let plan = ArrivalPlan { choices: vec![ArrivalChoice::Points { order: id.clone(), recipient: recipient.clone(), location: cairo.clone() }] };
    let before = serde_json::to_vec(&state).unwrap();
    let unit_before = serde_json::to_vec(&state.land.units[&recipient]).unwrap();
    let prepared = arrival::prepare(&content, &state, SeatId::new(Side::Commonwealth, Role::Logistics), &plan, &quota(), false).unwrap();
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    for role in [Role::Commander, Role::FrontLine, Role::RearArea, Role::Air] {
        assert!(matches!(arrival::prepare(&content, &state, SeatId::new(Side::Commonwealth, role), &plan, &quota(), false), Err(Rejection::Illegal { .. })));
    }
    accept(&mut state, &plan);
    let mut restored: crate::State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    let next = arrival::finish(&content, &mut restored, &prepared).unwrap();
    assert_eq!(next.infantry_city.as_deref(), Some("cairo"));
    let own = side(&restored, Side::Commonwealth).unwrap();
    assert!(own.orders.is_empty());
    assert_eq!(own.next_serial, 1);
    assert_eq!(own.counters.len(), 1);
    let counter = &own.counters[0];
    assert_eq!((&counter.id, &counter.recipient, &counter.location, counter.points), (&id, &recipient, &cairo, 1));
    assert!(matches!(counter.training, Training::Training { completed: 0, candidate: None, last_counted: None, .. }));
    assert!(!counter.physical.on_network);
    assert!(counter.physical.activity.is_none() && counter.physical.funding.is_none() && counter.carriage.is_none());
    assert_eq!(counter.physical.cp_quarters, 0);
    assert_eq!(serde_json::to_vec(&restored.land.units[&recipient]).unwrap(), unit_before);
    let finished = serde_json::to_vec(&restored).unwrap();
    assert!(arrival::finish(&content, &mut restored, &prepared).is_err());
    assert_eq!(serde_json::to_vec(&restored).unwrap(), finished);
}

/// Cases: land:20.78A, land:20.78B
#[test]
fn quotas_use_retained_gt_total_and_city_after_previous_equipment_has_gone() {
    let (content, mut state, recipient, cairo, alexandria) = fixture();
    let truck = truck_order(&mut state, 2);
    let foot = foot_order(&mut state);
    let plan = ArrivalPlan { choices: vec![
        ArrivalChoice::Trucks { order: truck, location: alexandria.clone(), pool_target: ProductionPoolTarget::Create },
        ArrivalChoice::Points { order: foot, recipient: recipient.clone(), location: cairo.clone() },
    ] };
    let prepared = arrival::prepare(&content, &state, SeatId::new(Side::Commonwealth, Role::Logistics), &plan, &quota(), false).unwrap();
    accept(&mut state, &plan);
    let retained = arrival::finish(&content, &mut state, &prepared).unwrap();
    assert_eq!((retained.total_trucks, retained.alexandria_trucks), (8, 2));
    assert_eq!(retained.infantry_city.as_deref(), Some("cairo"));
    // Current mobile/lossy holdings cannot recover either historical limit.
    state.logistics.truck_pools.clear();
    side_mut(&mut state, Side::Commonwealth).unwrap().counters.clear();
    side_mut(&mut state, Side::Commonwealth).unwrap().arrival.phase = ProcedurePhase::Awaiting { submitted: None };
    let truck = truck_order(&mut state, 1);
    let foot = foot_order(&mut state);
    let wrong = ArrivalPlan { choices: vec![
        ArrivalChoice::Trucks { order: truck.clone(), location: alexandria.clone(), pool_target: ProductionPoolTarget::Create },
        ArrivalChoice::Points { order: foot.clone(), recipient: recipient.clone(), location: cairo.clone() },
    ] };
    let before = serde_json::to_vec(&state).unwrap();
    assert!(arrival::prepare(&content, &state, SeatId::new(Side::Commonwealth, Role::Logistics), &wrong, &retained, false).is_err());
    let wrong = ArrivalPlan { choices: vec![
        ArrivalChoice::Trucks { order: truck, location: cairo, pool_target: ProductionPoolTarget::Create },
        ArrivalChoice::Points { order: foot, recipient, location: alexandria },
    ] };
    assert!(arrival::prepare(&content, &state, SeatId::new(Side::Commonwealth, Role::Logistics), &wrong, &retained, false).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
}

/// Cases: land:20.41, land:20.78A
#[test]
fn late_pool_identity_exhaustion_rolls_back_preceding_counter_and_order_removal() {
    let (content, mut state, recipient, cairo, _) = fixture();
    let foot = foot_order(&mut state);
    let truck = truck_order(&mut state, 1);
    let plan = ArrivalPlan { choices: vec![
        ArrivalChoice::Points { order: foot, recipient, location: cairo.clone() },
        ArrivalChoice::Trucks { order: truck, location: cairo, pool_target: ProductionPoolTarget::Create },
    ] };
    state.logistics.truck_pool_serial.insert(Side::Commonwealth, u64::MAX);
    let prepared = arrival::prepare(&content, &state, SeatId::new(Side::Commonwealth, Role::Logistics), &plan, &quota(), false).unwrap();
    accept(&mut state, &plan);
    let before = serde_json::to_vec(&state).unwrap();
    assert!(matches!(arrival::finish(&content, &mut state, &prepared), Err(EngineError::Invariant { detail }) if detail.contains("identity space exhausted")));
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    assert!(side(&state, Side::Commonwealth).unwrap().counters.is_empty());
    assert_eq!(side(&state, Side::Commonwealth).unwrap().orders.len(), 2);
}

/// Cases: land:20.41, land:20.63, land:20.78A
#[test]
fn duplicate_orders_and_existing_pool_adapter_and_wrong_nation_cannot_gain_delivery() {
    let (content, mut state, recipient, cairo, _) = fixture();
    let id = foot_order(&mut state);
    let foreign = state.land.units.values().find(|unit| unit.side == Side::Axis).unwrap().id.clone();
    let wrong = ArrivalPlan { choices: vec![ArrivalChoice::Points { order: id.clone(), recipient: foreign, location: cairo.clone() }] };
    assert!(matches!(arrival::prepare(&content, &state, SeatId::new(Side::Commonwealth, Role::Logistics), &wrong, &quota(), false), Err(Rejection::Illegal { .. })));
    let own = side_mut(&mut state, Side::Commonwealth).unwrap();
    own.orders.push(own.orders[0].clone());
    let duplicate = ArrivalPlan { choices: vec![ArrivalChoice::Points { order: id, recipient, location: cairo.clone() }; 2] };
    let before = serde_json::to_vec(&state).unwrap();
    assert!(matches!(arrival::prepare(&content, &state, SeatId::new(Side::Commonwealth, Role::Logistics), &duplicate, &quota(), false), Err(Rejection::Engine(EngineError::Invariant { .. }))));
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    side_mut(&mut state, Side::Commonwealth).unwrap().orders.clear();
    let id = truck_order(&mut state, 1);
    let merge = ArrivalPlan { choices: vec![ArrivalChoice::Trucks { order: id, location: cairo, pool_target: ProductionPoolTarget::Existing { pool: "no-fabricated-funding".into() } }] };
    assert!(matches!(arrival::prepare(&content, &state, SeatId::new(Side::Commonwealth, Role::Logistics), &merge, &quota(), false), Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:20.78A"));
}
