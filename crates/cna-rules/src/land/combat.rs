//! Private whole-unit gun positions, fixed for one Combat Segment.
use super::formation;
use crate::{
    CnaContent, State, ownership,
    state::Pending,
    steps::{illegal, open},
};
use cna_content::units::Toe;
use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    ids::{SeatId, UnitId},
};
use cna_protocol::{Role, Side};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub mod barrage;
pub mod retreat;

pub const POSITION_KIND: &str = "cna.combat.position";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    #[default]
    Forward,
    Back,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PositionOrder {
    pub unit: UnitId,
    pub position: Position,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatState {
    #[serde(default)]
    pub barrage: barrage::BarrageState,
    #[serde(default)]
    pub retreat: retreat::RetreatState,
    pub pinned: BTreeSet<UnitId>,
    pub cp_charged: BTreeMap<UnitId, i32>,
    pub positions: BTreeMap<UnitId, Position>,
    pub position_orders: BTreeMap<SeatId, Vec<PositionOrder>>,
    pub positions_locked: bool,
}

/// Any unit carrying barrage-capable TOE chooses a whole-unit position.
/// Empty parent headquarters do not acquire a position merely by representing children.
/// Cases: land:11.12, land:12.0, land:12.11, land:12.16
pub fn has_position(content: &CnaContent, state: &State, id: &UnitId) -> bool {
    let Some(unit) = state.land.units.get(id) else {
        return false;
    };
    if unit.location.hex().is_none()
        || crate::view::toe_points(content, unit).is_none_or(|n| n <= 0)
    {
        return false;
    }
    let Some(class) = formation::class(content, id) else {
        return false;
    };
    if class.barrage.is_some_and(|n| n > 0) {
        return true;
    }
    match &unit.toe {
        Some(Toe::Weapons(points)) => points.iter().any(|p| {
            p.n > 0
                && content
                    .units
                    .weapons
                    .get(&p.weapon)
                    .is_some_and(|w| w.barrage.is_some_and(|n| n > 0))
        }),
        _ => false,
    }
}

fn available(content: &CnaContent, state: &State, seat: SeatId) -> Vec<UnitId> {
    state
        .units_of(seat.side)
        .filter(|u| {
            has_position(content, state, &u.id)
                && ownership::seat_for_unit(content, state, &u.id) == seat.role
        })
        .map(|u| u.id.clone())
        .collect()
}

/// Always create the same land-role windows, including empty ones. Hidden enemy gun presence
/// cannot change the number of decision identities observed by a controller.
/// Positions remain private after the simultaneous window closes.
/// Cases: land:3.6, land:12.11, land:12.12
pub fn enter_positions(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    state.land.combat.retreat = retreat::RetreatState::default();
    state.land.combat.position_orders.clear();
    state.land.combat.barrage.targets.clear();
    state.land.combat.barrage.plans.clear();
    state.land.combat.pinned.clear();
    state.land.combat.cp_charged.clear();
    state.land.combat.positions_locked = false;
    for side in Side::ALL {
        for role in [Role::FrontLine, Role::RearArea, Role::Logistics] {
            let seat = SeatId::new(side, role);
            let ids = available(content, state, seat);
            let count = u32::try_from(ids.len()).map_err(|_| EngineError::Invariant {
                detail: "too many gun units for a position window".into(),
            })?;
            let space = ActionSpace::new(ActionSchema::List {
                min: count, max: count,
                item: Box::new(ActionSchema::Record { fields: vec![
                    FieldSchema { name: "unit".into(), doc: "One whole gun unit commanded by this seat; include each eligible unit once.".into(), schema: ActionSchema::Unit { among: ids }, optional: false },
                    FieldSchema { name: "position".into(), doc: "Private position fixed through this Combat Segment.".into(), schema: ActionSchema::Choice { options: vec![
                        ChoiceOption { id: "forward".into(), label: "Forward".into(), detail: Some("Coordinate and split barrage fire, with exposure to assault.".into()) },
                        ChoiceOption { id: "back".into(), label: "Back".into(), detail: Some("Barrage independently without splitting; excluded from anti-armor and close assault.".into()) },
                    ]}, optional: false },
                ]}),
            }).with_pass("Place every eligible gun unit Forward for this segment.");
            open(state, cx, seat, POSITION_KIND, "Choose private whole-unit gun positions; passing explicitly chooses Forward for all your guns.".into(), &["land:12.11", "land:12.12", "land:12.16"], Trigger::Scheduled, Secrecy::SecretSimultaneous, space);
        }
    }
    Ok(())
}

/// Store a complete private declaration. Nothing changes positions midway through the window.
/// Cases: land:3.6, land:12.11, land:12.12, land:12.16
pub fn answer_positions(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    _cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let required: BTreeSet<_> = available(content, state, pending.seat)
        .into_iter()
        .collect();
    let orders: Vec<PositionOrder> = if action.is_null() {
        required
            .iter()
            .map(|id| PositionOrder {
                unit: id.clone(),
                position: Position::Forward,
            })
            .collect()
    } else {
        serde_json::from_value(action.clone())
            .map_err(|_| illegal("expected complete gun position orders"))?
    };
    let selected: BTreeSet<_> = orders.iter().map(|o| o.unit.clone()).collect();
    if selected.len() != orders.len() || selected != required {
        return Err(illegal("include each eligible own gun unit exactly once"));
    }
    if state.land.combat.positions_locked
        || state
            .land
            .combat
            .position_orders
            .contains_key(&pending.seat)
    {
        return Err(illegal("gun positions already committed for this window"));
    }
    state
        .land
        .combat
        .position_orders
        .insert(pending.seat, orders);
    if !state
        .decisions
        .pending
        .iter()
        .any(|p| p.kind == POSITION_KIND)
    {
        let mut positions = BTreeMap::new();
        for orders in state.land.combat.position_orders.values() {
            for order in orders {
                positions.insert(order.unit.clone(), order.position);
            }
        }
        state.land.combat.positions = positions;
        state.land.combat.position_orders.clear();
        state.land.combat.positions_locked = true;
        for id in state.land.combat.positions.keys() {
            let u = &state.land.units[id];
            let mut unit = crate::view::unit_view(content, u);
            stamp_view(state, id, &mut unit);
            _cx.emit(cna_core::event::EngineEvent::new(
                cna_core::visibility::Audience::Side(u.side),
                cna_protocol::GameEvent::UnitUpdated { unit },
            ));
        }
    }
    Ok("Private gun positions committed.".into())
}
/// A complete, unique declaration from the request's own enumeration. No enemy state is read.
/// Randomness is supplied by the controller, never by campaign adjudication.
/// Cases: land:3.6, land:12.11, land:12.12, land:12.16
pub fn random_positions(
    request: &cna_core::decision::DecisionRequest,
    rng: &mut cna_core::dice::CampaignRng,
) -> Value {
    if request.kind != POSITION_KIND {
        return Value::Null;
    }
    let ActionSchema::List { item, .. } = &request.space.schema else {
        return Value::Null;
    };
    let ActionSchema::Record { fields } = item.as_ref() else {
        return Value::Null;
    };
    let Some(ActionSchema::Unit { among }) =
        fields.iter().find(|f| f.name == "unit").map(|f| &f.schema)
    else {
        return Value::Null;
    };
    let ids: BTreeSet<_> = among.iter().cloned().collect();
    serde_json::to_value(
        ids.into_iter()
            .map(|unit| PositionOrder {
                unit,
                position: if rng.d6().value() <= 3 {
                    Position::Forward
                } else {
                    Position::Back
                },
            })
            .collect::<Vec<_>>(),
    )
    .expect("position orders are serializable")
}

/// Private status fields use the same owning-side audience as the unit itself.
/// Cases: land:3.6, land:12.12, land:12.44
pub(crate) fn stamp_view(state: &State, id: &UnitId, unit: &mut cna_protocol::UnitView) {
    if let Some(detail) = unit.detail.as_mut() {
        if let Some(position) = state.land.combat.positions.get(id) {
            detail.insert("gun_position".into(), serde_json::json!(position));
        }
        detail.insert(
            "combat_pinned".into(),
            serde_json::json!(state.land.combat.pinned.contains(id)),
        );
    }
}
/// The development profile skips the remaining assault procedure, but pinning still expires
/// at the end of its Combat Segment rather than persisting into the next Movement Segment.
/// Cases: land:12.44
pub(crate) fn finish_pins(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) {
    let pinned = std::mem::take(&mut state.land.combat.pinned);
    for id in pinned {
        if let Some(u) = state.land.units.get(&id) {
            let mut unit = crate::view::unit_view(content, u);
            stamp_view(state, &id, &mut unit);
            cx.emit(cna_core::event::EngineEvent::new(
                cna_core::visibility::Audience::Side(u.side),
                cna_protocol::GameEvent::UnitUpdated { unit },
            ));
        }
    }
}
// Draft integration tests for combat position window; install with combat.rs and dispatch glue.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Cna,
        seq::{Block, Half},
        state::Location,
    };
    use cna_core::{
        decision::DecisionResponse,
        dice::CampaignRng,
        engine::{Command, Game, Ruleset, evaluate},
        visibility::Perspective,
    };
    use serde_json::json;
    use std::sync::OnceLock;

    fn content() -> &'static CnaContent {
        static CONTENT: OnceLock<CnaContent> = OnceLock::new();
        CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
    }
    fn game() -> Game<Cna> {
        let c = content();
        let mut state = State::new(c).unwrap();
        // Real roster and weapon values; only positions are overlaid for this procedure fixture.
        for side in Side::ALL {
            let id = state.units_of(side).filter(|u| formation::strength(c, &state, &u.id) > 0 && (
                formation::class(c, &u.id).is_some_and(|cl| cl.barrage.is_some_and(|n| n>0)) ||
                matches!(&u.toe, Some(Toe::Weapons(w)) if w.iter().any(|p| p.n>0 && c.units.weapons[&p.weapon].barrage.is_some_and(|n| n>0)))
            ) && ownership::seat_for_unit(c, &state, &u.id) == Role::FrontLine).map(|u|u.id.clone()).next().unwrap();
            state.land.units.get_mut(&id).unwrap().location = Location::Hex {
                hex: if side == Side::Axis { "C4218" } else { "C4219" }.into(),
            };
        }
        state.cursor.block = Block::PlayerHalf;
        state.cursor.half = Some(Half::A);
        state.cursor.index = 3;
        state.cursor.op_stage = Some(1);
        state.cursor.entered = false;
        state.turn.player_a = Some(Side::Axis);
        Game {
            state,
            rng: CampaignRng::from_seed([9; 32]).state(),
        }
    }
    fn open_game() -> Game<Cna> {
        evaluate(&Cna::dev(), content(), &game(), &Command::Advance)
            .unwrap()
            .game
    }
    fn response(game: &Game<Cna>, seat: SeatId, action: Value) -> Command {
        let p = Cna::dev()
            .pending(content(), &game.state)
            .into_iter()
            .find(|p| p.seat == seat && p.kind == POSITION_KIND)
            .unwrap();
        Command::Respond(DecisionResponse {
            decision_id: p.id.clone(),
            seat,
            controller_epoch: 1,
            decision_revision: p.revision,
            idempotency_key: p.id.to_string(),
            action,
            public_explanation: None,
        })
    }
    fn declaration(state: &State, seat: SeatId, position: Position) -> Value {
        json!(
            available(content(), state, seat)
                .into_iter()
                .map(|id| PositionOrder { unit: id, position })
                .collect::<Vec<_>>()
        )
    }
    /// Cases: land:12.11, land:12.12, land:3.6
    #[test]
    fn position_window_is_simultaneous_private_and_checkpointable() {
        let mut game = open_game();
        assert_eq!(game.state.decisions.pending.len(), 6);
        let seat = SeatId::new(Side::Axis, Role::FrontLine);
        let orders = declaration(&game.state, seat, Position::Back);
        let t = evaluate(
            &Cna::dev(),
            content(),
            &game,
            &response(&game, seat, orders),
        )
        .unwrap();
        assert_eq!(t.game.rng, game.rng);
        assert!(t.game.state.land.combat.positions.is_empty());
        assert!(!t.game.state.land.combat.positions_locked);
        for event in &t.events {
            assert!(!Perspective::Side(Side::Commonwealth).can_see(&event.audience));
        }
        let encoded = serde_json::to_value(&t.game.state).unwrap();
        let restored: State = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), encoded);
        let own = Cna::dev().observe(content(), &t.game.state, Perspective::Side(Side::Axis));
        let enemy = Cna::dev().observe(
            content(),
            &t.game.state,
            Perspective::Side(Side::Commonwealth),
        );
        let operator = Cna::dev().observe(content(), &t.game.state, Perspective::Operator);
        assert!(
            !own["combat"]["position_orders"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        assert!(
            enemy["combat"]["position_orders"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            operator["combat"]["position_orders"],
            own["combat"]["position_orders"]
        );
        game = t.game;
        let seats: Vec<_> = game
            .state
            .decisions
            .pending
            .iter()
            .map(|p| p.seat)
            .collect();
        for seat in seats {
            game = evaluate(
                &Cna::dev(),
                content(),
                &game,
                &response(&game, seat, Value::Null),
            )
            .unwrap()
            .game;
        }
        assert!(game.state.land.combat.positions_locked);
        assert!(game.state.land.combat.position_orders.is_empty());
        for id in available(
            content(),
            &game.state,
            SeatId::new(Side::Axis, Role::FrontLine),
        ) {
            assert_eq!(game.state.land.combat.positions[&id], Position::Back);
        }
        for id in available(
            content(),
            &game.state,
            SeatId::new(Side::Commonwealth, Role::FrontLine),
        ) {
            assert_eq!(game.state.land.combat.positions[&id], Position::Forward);
        }
    }
    /// Cases: land:12.11, land:12.12
    #[test]
    fn incomplete_duplicate_foreign_and_malformed_positions_do_not_change_game_or_dice() {
        let game = open_game();
        let seat = SeatId::new(Side::Axis, Role::FrontLine);
        let ids = available(content(), &game.state, seat);
        assert!(!ids.is_empty());
        let foreign = available(
            content(),
            &game.state,
            SeatId::new(Side::Commonwealth, Role::FrontLine),
        );
        assert!(!foreign.is_empty());
        let before = serde_json::to_value(&game).unwrap();
        for action in [
            json!([]),
            json!([{"unit":ids[0],"position":"back"},{"unit":ids[0],"position":"forward"}]),
            json!([{"unit":foreign[0],"position":"back"}]),
            json!([{"unit":ids[0],"position":"sideways"}]),
            json!([{"unit":ids[0],"position":"back","extra":true}]),
        ] {
            assert!(
                evaluate(
                    &Cna::dev(),
                    content(),
                    &game,
                    &response(&game, seat, action)
                )
                .is_err()
            );
            assert_eq!(serde_json::to_value(&game).unwrap(), before);
        }
    }
    /// Cases: land:3.6, land:12.12
    #[test]
    fn enemy_gun_population_cannot_change_position_window_identity_counts() {
        let full = game();
        let mut fewer = full.clone();
        for u in fewer
            .state
            .land
            .units
            .values_mut()
            .filter(|u| u.side == Side::Commonwealth)
        {
            u.toe = None;
        }
        let full = evaluate(&Cna::dev(), content(), &full, &Command::Advance)
            .unwrap()
            .game;
        let fewer = evaluate(&Cna::dev(), content(), &fewer, &Command::Advance)
            .unwrap()
            .game;
        let own = |s: &State| {
            s.decisions
                .pending
                .iter()
                .filter(|p| p.seat.side == Side::Axis)
                .map(|p| (p.id.clone(), p.space.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(own(&full.state), own(&fewer.state));
        assert_eq!(full.state.decisions.pending.len(), 6);
        assert_eq!(fewer.state.decisions.pending.len(), 6);
    }
    /// Cases: land:3.6, land:12.11, land:12.12
    #[test]
    fn position_submission_order_does_not_change_final_game() {
        let initial = open_game();
        let seats: Vec<_> = initial
            .state
            .decisions
            .pending
            .iter()
            .map(|p| p.seat)
            .collect();
        let run = |seats: Vec<SeatId>| {
            let mut game = initial.clone();
            for seat in seats {
                let position = if seat.side == Side::Axis {
                    Position::Back
                } else {
                    Position::Forward
                };
                let action = declaration(&game.state, seat, position);
                game = evaluate(
                    &Cna::dev(),
                    content(),
                    &game,
                    &response(&game, seat, action),
                )
                .unwrap()
                .game;
            }
            serde_json::to_value(game).unwrap()
        };
        assert_eq!(run(seats.clone()), run(seats.into_iter().rev().collect()));
    }

    /// Cases: land:12.11, land:12.12, land:12.16
    #[test]
    fn baseline_positions_are_complete_legal_and_use_only_controller_rng() {
        let initial = open_game();
        for seed in 0..96u8 {
            let mut game = initial.clone();
            let mut local = CampaignRng::from_seed([seed; 32]);
            while !game.state.decisions.pending.is_empty() {
                let request = Cna::dev().pending(content(), &game.state)[0].clone();
                let unchanged = serde_json::to_value(&game).unwrap();
                let action = random_positions(&request, &mut local);
                assert_eq!(serde_json::to_value(&game).unwrap(), unchanged);
                game = evaluate(
                    &Cna::dev(),
                    content(),
                    &game,
                    &response(&game, request.seat, action),
                )
                .unwrap()
                .game;
                assert_eq!(game.rng, initial.rng);
            }
            assert!(game.state.land.combat.positions_locked);
        }
    }
}
