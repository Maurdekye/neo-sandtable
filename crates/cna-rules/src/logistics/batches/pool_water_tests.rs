use super::*;
use cna_content::{scenario::Placement, units::Trucks};

fn pool_fixture() -> (State, String) {
    let mut s = fixture();
    s.logistics.truck_pools.clear();
    let id = crate::logistics::pools::add_truck_pool(
        &mut s.logistics,
        None,
        Side::Axis,
        Placement::Hex {
            hex: "C4020".into(),
        },
        Some(Location::Hex {
            hex: "C4020".into(),
        }),
        Trucks {
            medium: 4,
            ..Trucks::default()
        },
        Supplies {
            water: 999,
            ..Supplies::default()
        },
    )
    .unwrap();
    (s, id)
}
fn action(id: &str, n: i32) -> Value {
    json!({"allocations":[],"wells":[],"pool_allocations":[{"pool":id,"activity":n,
        "draws":[{"source":serde_json::to_string(&crate::logistics::SupplySource::Dump("test-stock".into())).unwrap(),"stores":0,"water":n}]}]})
}
fn open_game() -> (Game<Cna>, String) {
    let (mut s, id) = pool_fixture();
    let mut rng = CampaignRng::from_seed([11; 32]);
    enter_water(
        content(),
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
        false,
    )
    .unwrap();
    (
        Game {
            state: s,
            rng: rng.state(),
        },
        id,
    )
}

/// Cases: airlog:52.42, land:29.34, land:29.35, land:3.6
#[test]
fn real_respond_checkpoint_and_advance_issue_once_without_respond_stock_effects() {
    let (game, id) = open_game();
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let command = response(&p, action(&id, 4));
    let first = evaluate(&Cna::dev(), content(), &game, &command).unwrap();
    assert_eq!(first.game.rng, game.rng);
    assert_eq!(
        first.game.state.logistics.dumps["test-stock"]
            .supplies
            .water,
        100
    );
    assert_eq!(
        first.game.state.logistics.truck_pools[0]
            .activity_water
            .get(),
        0
    );
    let mut expected = game.state.logistics.clone();
    expected.water_window = first.game.state.logistics.water_window.clone();
    assert_eq!(
        serde_json::to_value(expected).unwrap(),
        serde_json::to_value(&first.game.state.logistics).unwrap()
    );
    assert!(evaluate(&Cna::dev(), content(), &first.game, &command).is_err());
    let mut checkpoint: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&first.game).unwrap()).unwrap();
    let cw = checkpoint
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Commonwealth)
        .unwrap()
        .clone();
    checkpoint = evaluate(
        &Cna::dev(),
        content(),
        &checkpoint,
        &response(&cw, Value::Null),
    )
    .unwrap()
    .game;
    let completed = evaluate(&Cna::dev(), content(), &checkpoint, &Command::Advance)
        .unwrap()
        .game;
    assert_eq!(completed.rng, game.rng);
    assert_eq!(
        completed.state.logistics.dumps["test-stock"].supplies.water,
        96
    );
    assert_eq!(
        completed.state.logistics.truck_pools[0]
            .activity_water
            .get(),
        4
    );
    assert_eq!(completed.state.logistics.truck_pools[0].cargo.water, 999);
    assert_eq!(
        completed.state.cursor.anchor(),
        "opstage.organization.water_distribution"
    );
    assert!(
        completed
            .state
            .decisions
            .pending
            .iter()
            .all(|p| p.kind == WELL_ALLOCATION)
    );
    let mut state = completed.state.clone();
    let before = serde_json::to_value(&state).unwrap();
    let mut rng = CampaignRng::from_state(&completed.rng);
    let mut events = vec![];
    finish_water(
        content(),
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert!(events.is_empty());
    assert_eq!(rng.state(), completed.rng);
    let legacy: WaterAnswer = serde_json::from_value(json!({"allocations":[],"wells":[]})).unwrap();
    assert!(legacy.pool_allocations.is_empty());
}

/// Cases: airlog:52.42, land:29.35, land:3.6
#[test]
fn joint_unit_pool_overdraw_and_late_invalid_row_reject_entire_answer() {
    let (mut game, id) = open_game();
    game.state
        .logistics
        .dumps
        .get_mut("test-stock")
        .unwrap()
        .supplies
        .water = 4;
    let p = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let mut shared = action(&id, 4);
    shared["allocations"] = json!([{"unit":AX,"infantry":1,"activity":0,"pasta":false,
        "draws":[{"source":serde_json::to_string(&crate::logistics::SupplySource::Dump("test-stock".into())).unwrap(),"stores":0,"water":1}]}]);
    let before = serde_json::to_value(&game).unwrap();
    assert!(evaluate(&Cna::dev(), content(), &game, &response(&p, shared)).is_err());
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    let mut bad = action("unknown", 1);
    bad["allocations"] = json!([{"unit":AX,"infantry":1,"activity":0,"pasta":false,
        "draws":[{"source":serde_json::to_string(&crate::logistics::SupplySource::Dump("test-stock".into())).unwrap(),"stores":0,"water":1}]}]);
    assert!(evaluate(&Cna::dev(), content(), &game, &response(&p, bad)).is_err());
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
}

/// Cases: airlog:52.42, land:29.35, land:3.6
#[test]
fn observer_stream_pair_and_typed_closure_error_preserve_checkpoint_input() {
    let (a, id) = open_game();
    let mut b = a.clone();
    b.state
        .logistics
        .dumps
        .get_mut("test-stock")
        .unwrap()
        .supplies
        .water = 200;
    let p = a
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let command = response(&p, action(&id, 4));
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &a,
        &b,
        &command,
        Side::Commonwealth,
    );
    let mut closed = vec![];
    for game in [a, b] {
        let ax = evaluate(&Cna::dev(), content(), &game, &command)
            .unwrap()
            .game;
        let cw = ax
            .state
            .decisions
            .pending
            .iter()
            .find(|p| p.seat.side == Side::Commonwealth)
            .unwrap()
            .clone();
        closed.push(
            evaluate(&Cna::dev(), content(), &ax, &response(&cw, Value::Null))
                .unwrap()
                .game,
        );
    }
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &closed[0],
        &closed[1],
        &Command::Advance,
        Side::Commonwealth,
    );
    let mut broken = closed.remove(0);
    broken.state.turn.weather = None;
    let before = serde_json::to_value(&broken).unwrap();
    assert!(
        matches!(evaluate(&Cna::dev(),content(),&broken,&Command::Advance),Err(Rejection::Engine(EngineError::Unsupported{case,..})) if case=="land:29.1")
    );
    assert_eq!(serde_json::to_value(&broken).unwrap(), before);
}

/// Cases: airlog:52.42, land:29.34, land:29.35, land:3.6
#[test]
fn fallible_baseline_adds_exact_pool_row_on_same_owner_stock_draft() {
    let (game, id) = open_game();
    let request = Cna::dev()
        .pending(content(), &game.state)
        .into_iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap();
    let before = serde_json::to_value(&game).unwrap();
    let mut controller = CampaignRng::from_seed([19; 32]);
    let answer = crate::logistics::baseline::logistics_orders_with_profile(
        content(),
        &game.state,
        &request,
        &mut controller,
        false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(answer["pool_allocations"].as_array().unwrap().len(), 1);
    assert_eq!(answer["pool_allocations"][0]["pool"], id);
    assert_eq!(answer["pool_allocations"][0]["activity"], 4);
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    let pending = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap();
    let recorded = evaluate(&Cna::dev(), content(), &game, &response(pending, answer))
        .unwrap()
        .game;
    assert_eq!(
        recorded.state.logistics.dumps["test-stock"].supplies.water,
        100
    );
    assert_eq!(
        recorded.state.logistics.truck_pools[0].activity_water.get(),
        0
    );
    let mut broken = game.state.clone();
    broken.turn.weather = None;
    assert!(
        matches!(crate::logistics::baseline::logistics_orders_with_profile(content(),&broken,&request,&mut controller,false),Err(EngineError::Unsupported{case,..}) if case=="land:29.1")
    );
}
