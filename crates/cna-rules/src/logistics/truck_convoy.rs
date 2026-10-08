//! Whole-pool convoy procedure with answer-only buffering and atomic closure.
//! Whole real pools, one immutable Move per pool, coastal closure precedes this batch.
use super::{
    CargoPacking, FuelCohortSelection, FuelTruckKind, SupplyError, TruckFuelCohort, box_handling,
    cargo_history, pool_fuel, segment, weather,
};
use crate::{
    CnaContent, State,
    land::{breakdown, convoy_move},
    seq::Half,
    state::{Location, Pending, TruckPool},
    steps::{illegal, open},
};
use cargo_history::{CargoError, CargoSite, motion};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{HexId, SeatId},
    quantity::{FuelTenths, WaterPoints},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::{airlog::trucks::TruckType, land::weather::WeatherKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "cna.logistics.truck_convoy.batch";
pub const ANCHOR: &str = "opstage.truck_convoy_movement";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConvoyClosureKey {
    pub game_turn: u16,
    pub op_stage: u8,
    pub half: Half,
    pub cycle: u16,
    pub side: Side,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum TruckConvoyOrder {
    Move { pool: String, path: Vec<HexId> },
}
impl TruckConvoyOrder {
    fn parts(&self) -> (&str, &[HexId]) {
        match self {
            Self::Move { pool, path } => (pool, path),
        }
    }
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct TruckConvoyProcedure {
    pub key: Option<ConvoyClosureKey>,
    pub submitted: Option<Vec<TruckConvoyOrder>>,
    pub next_order: u32,
    pub waiting_pool: Option<String>,
    pub resolved: bool,
}
impl<'de> Deserialize<'de> for TruckConvoyProcedure {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Wire {
            key: Option<ConvoyClosureKey>,
            submitted: Option<Vec<TruckConvoyOrder>>,
            next_order: u32,
            waiting_pool: Option<String>,
            resolved: bool,
        }
        let w = Wire::deserialize(d)?;
        let p = Self {
            key: w.key,
            submitted: w.submitted,
            next_order: w.next_order,
            waiting_pool: w.waiting_pool,
            resolved: w.resolved,
        };
        p.validate().map_err(serde::de::Error::custom)?;
        Ok(p)
    }
}
fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("truck convoy: {detail}"),
    }
}
fn unsupported(case: &str, detail: &str) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    }
}
fn accepted(error: Rejection) -> EngineError {
    match error {
        Rejection::Engine(e) => e,
        _ => invariant("accepted plan no longer validates"),
    }
}
impl TruckConvoyProcedure {
    fn validate(&self) -> Result<(), String> {
        if self
            .key
            .as_ref()
            .is_some_and(|k| k.game_turn == 0 || !(1..=3).contains(&k.op_stage) || k.cycle == 0)
        {
            return Err("invalid convoy closure key".into());
        }
        let Some(_) = &self.key else {
            return if self.submitted.is_none()
                && self.next_order == 0
                && self.waiting_pool.is_none()
                && !self.resolved
            {
                Ok(())
            } else {
                Err("convoy progress without key".into())
            };
        };
        if self.resolved {
            if self.submitted.is_some() || self.next_order != 0 || self.waiting_pool.is_some() {
                return Err("resolved convoy retains execution progress".into());
            }
            return Ok(());
        }
        let Some(orders) = &self.submitted else {
            return if self.next_order == 0 && self.waiting_pool.is_none() {
                Ok(())
            } else {
                Err("convoy execution without accepted list".into())
            };
        };
        let mut pools = BTreeSet::new();
        for order in orders {
            let (pool, path) = order.parts();
            if pool.is_empty() || !pools.insert(pool) || path.len() > 4096 {
                return Err("invalid or duplicate convoy order".into());
            }
        }
        let next = usize::try_from(self.next_order).map_err(|_| "convoy index overflow")?;
        if next > orders.len() {
            return Err("convoy index exceeds list".into());
        }
        if let Some(waiting) = &self.waiting_pool
            && (next == 0 || orders[next - 1].parts().0 != waiting)
        {
            return Err("convoy wait does not bind the executed order".into());
        }
        Ok(())
    }
}
fn key(s: &State) -> Result<ConvoyClosureKey, EngineError> {
    if s.cursor.anchor() != ANCHOR {
        return Err(invariant("wrong convoy anchor"));
    }
    Ok(ConvoyClosureKey {
        game_turn: s.cursor.game_turn,
        op_stage: s
            .cursor
            .op_stage
            .filter(|op| (1..=3).contains(op))
            .ok_or_else(|| invariant("missing OpStage"))?,
        half: s
            .cursor
            .half
            .ok_or_else(|| invariant("missing player half"))?,
        cycle: s.cursor.cycle,
        side: s
            .cursor
            .phasing(s.turn.player_a)
            .ok_or_else(|| invariant("missing phasing side"))?,
    })
}
fn pool<'a>(s: &'a State, side: Side, id: &str) -> Result<&'a TruckPool, Rejection> {
    if !s
        .logistics
        .truck_pools
        .iter()
        .any(|p| p.id == id && p.side == side)
    {
        return Err(illegal("unknown own convoy pool"));
    }
    let mut found = s.logistics.truck_pools.iter().filter(|p| p.id == id);
    let p = found
        .next()
        .ok_or_else(|| Rejection::Engine(invariant("missing pool")))?;
    if id.is_empty() || found.next().is_some() || !s.logistics.truck_pool_ids.contains(id) {
        return Err(Rejection::Engine(invariant("invalid real pool identity")));
    }
    Ok(p)
}
fn supply(error: SupplyError) -> Rejection {
    match error {
        SupplyError::Unsupported { case } => {
            Rejection::Engine(unsupported(case, "convoy supply source is unsupported"))
        }
        SupplyError::Insufficient => illegal("convoy cannot fund this path"),
        _ => Rejection::Engine(invariant("invalid convoy supply/fuel record")),
    }
}
fn cargo(error: CargoError) -> Rejection {
    match error {
        CargoError::ChoiceRequired => Rejection::Engine(unsupported(
            "airlog:53.25",
            "distinct cargo histories need a later handling slice",
        )),
        CargoError::Ceiling => {
            illegal("convoy cargo has exhausted its first carrier ceiling (53.25)")
        }
        _ => Rejection::Engine(invariant("invalid convoy cargo history")),
    }
}
fn physical(error: motion::MotionError) -> Rejection {
    match error {
        motion::MotionError::Unknown => Rejection::Engine(unsupported(
            "airlog:53.25",
            "legacy physical timing awaits authoritative OpStage start",
        )),
        motion::MotionError::Mixed => Rejection::Engine(unsupported(
            "airlog:53.25",
            "mixed physical timing requires a later packing slice",
        )),
        _ => Rejection::Engine(invariant("invalid physical convoy timing")),
    }
}
fn ceiling(c: &CnaContent, trucks: Trucks) -> Result<i32, Rejection> {
    let mut limit = None;
    for (kind, count) in [
        (TruckType::Light, trucks.light),
        (TruckType::Medium, trucks.medium),
        (TruckType::Heavy, trucks.heavy),
    ] {
        if count < 0 {
            return Err(Rejection::Engine(invariant("negative pool trucks")));
        }
        if count > 0 {
            let quarters = c
                .tables
                .airlog
                .truck_characteristics
                .truck(kind)
                .cpa_supplies
                .checked_mul(4)
                .filter(|n| *n > 0)
                .ok_or_else(|| Rejection::Engine(invariant("invalid truck CPA chart")))?;
            limit = Some(limit.map_or(quarters, |prior: i32| prior.min(quarters)));
        }
    }
    limit.ok_or_else(|| illegal("convoy has no trucks"))
}
fn demand(c: &CnaContent, s: &State, side: Side, id: &str) -> Result<i32, Rejection> {
    let p = pool(s, side, id)?;
    let total = segment::truck_total(&p.trucks).map_err(supply)?;
    let weather = s.turn.weather.as_ref().ok_or_else(|| {
        Rejection::Engine(unsupported("land:29.1", "convoy stage weather is missing"))
    })?;
    let need = total
        .checked_mul(if weather.kind == WeatherKind::Hot {
            2
        } else {
            1
        })
        .ok_or_else(|| Rejection::Engine(invariant("convoy water demand overflow")))?;
    let _ = c;
    if p.activity_water.get() < 0 {
        return Err(Rejection::Engine(invariant("negative convoy reserve")));
    }
    if p.activity_water.get() < need {
        return Err(illegal("convoy lacks whole activity reserve (52.51)"));
    }
    Ok(need)
}
fn current(
    s: &State,
    side: Side,
    id: &str,
    cpa: i32,
) -> Result<(Vec<motion::PhysicalTrucks>, cargo_history::CarrierTiming), Rejection> {
    let groups = pool_fuel::pool_segment_fuel_cohorts(s, id).map_err(supply)?;
    let physicals: Vec<_> = groups.iter().map(motion::PhysicalTrucks::from).collect();
    let timing =
        motion::timing(s, side, &CargoSite::Pool(id.into()), &physicals, cpa).map_err(physical)?;
    Ok((physicals, timing))
}
fn uniform(s: &State, side: Side, id: &str, goods: Supplies) -> Result<(), Rejection> {
    if goods != Supplies::default() {
        cargo_history::select(s, side, &CargoSite::Pool(id.into()), goods, None).map_err(cargo)?;
    }
    Ok(())
}
fn validate_plan(
    c: &CnaContent,
    s: &State,
    side: Side,
    orders: &[TruckConvoyOrder],
    strict: bool,
) -> Result<(), Rejection> {
    let mut draft = s.clone();
    let mut seen = BTreeSet::new();
    for order in orders {
        let (id, path) = order.parts();
        if !seen.insert(id) {
            return Err(illegal("each convoy pool may have one Move per list"));
        }
        let p = pool(&draft, side, id)?.clone();
        let route = convoy_move::prepare_pool_route(c, &draft, side, id, path, strict)?;
        if path.is_empty() {
            continue;
        }
        if let Some(reason) =
            box_handling::blocks_movement(&draft, &box_handling::Carrier::Pool(id.into()))
        {
            return Err(illegal(reason));
        }
        let cpa = ceiling(c, p.trucks)?;
        let (_, timing) = current(&draft, side, id, cpa)?;
        uniform(&draft, side, id, p.cargo)?;
        let need = demand(c, &draft, side, id)?;
        let mut spent = timing.spent_cp_quarters;
        let mut segment_cp = draft
            .logistics
            .pool_fuel_segments
            .get(id)
            .filter(|l| l.segment == segment::SegmentKey::current(&draft))
            .map_or(0, |l| l.cp_quarters);
        for (to, cost) in route.path.iter().zip(&route.costs) {
            let (_, before) = current(&draft, side, id, cpa)?;
            spent = spent
                .checked_add(cost.cp_quarters)
                .ok_or_else(|| Rejection::Engine(invariant("preview CP overflow")))?;
            if spent > cpa {
                return Err(illegal("whole convoy path exceeds its truck CPA"));
            }
            segment_cp = segment_cp
                .checked_add(cost.cp_quarters)
                .ok_or_else(|| Rejection::Engine(invariant("preview fuel CP overflow")))?;
            pool_fuel::spend_pool_segment_fuel(c, &mut draft, id, segment_cp).map_err(supply)?;
            // Fuel cargo debit occurs before the corresponding surviving-goods CP charge.
            cargo_history::advance(
                &mut draft,
                side,
                &CargoSite::Pool(id.into()),
                before,
                cost.cp_quarters,
            )
            .map_err(cargo)?;
            let (groups, _) = current(&draft, side, id, cpa)?;
            motion::advance(
                &mut draft,
                side,
                &CargoSite::Pool(id.into()),
                &groups,
                cost.cp_quarters,
            )
            .map_err(physical)?;
            draft
                .logistics
                .truck_pools
                .iter_mut()
                .find(|p| p.id == id)
                .unwrap()
                .location = Some(Location::Hex { hex: to.clone() });
            convoy_move::record_pool_posture(&mut draft, id, cost.on_network)
                .map_err(Rejection::Engine)?;
        }
        draft
            .logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap()
            .activity_water = WaterPoints::new(p.activity_water.get() - need);
    }
    Ok(())
}
pub fn space(s: &State, side: Side) -> ActionSpace {
    let mut ids: Vec<_> = s
        .logistics
        .truck_pools
        .iter()
        .filter(|p| p.side == side && p.location.as_ref().and_then(Location::hex).is_some())
        .map(|p| p.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    let options = ids
        .iter()
        .map(|id| ChoiceOption {
            id: id.clone(),
            label: id.clone(),
            detail: None,
        })
        .collect();
    let field = |name: &str, doc: &str, schema| FieldSchema {
        name: name.into(),
        doc: doc.into(),
        schema,
        optional: false,
    };
    ActionSpace::new(ActionSchema::List {
        min: 0,
        max: u32::try_from(ids.len()).unwrap_or(u32::MAX),
        item: Box::new(ActionSchema::Record {
            fields: vec![
                field(
                    "operation",
                    "Whole-pool movement.",
                    ActionSchema::Choice {
                        options: vec![ChoiceOption {
                            id: "move".into(),
                            label: "Move".into(),
                            detail: None,
                        }],
                    },
                ),
                field(
                    "pool",
                    "A real own resolved pool; each once.",
                    ActionSchema::Choice { options },
                ),
                field(
                    "path",
                    "Consecutive mapped destinations, excluding the origin.",
                    ActionSchema::List {
                        item: Box::new(ActionSchema::Hex { among: None }),
                        min: 0,
                        max: 4096,
                    },
                ),
            ],
        }),
    })
    .with_context(json!({"anchor":ANCHOR,"side":side,"pass":[]}))
}
/// Record only a complete owner-known list; effects await the closure.
/// Cases: airlog:53.12, airlog:53.21, airlog:53.22, land:3.6
pub fn answer(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    action: &Value,
    strict: bool,
    _cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let expected = key(s).map_err(Rejection::Engine)?;
    s.logistics
        .truck_convoy
        .validate()
        .map_err(|e| Rejection::Engine(invariant(&e)))?;
    if p.seat != SeatId::new(expected.side, Role::Logistics)
        || p.kind != KIND
        || s.logistics.truck_convoy.key.as_ref() != Some(&expected)
    {
        return Err(illegal("not this convoy batch"));
    }
    if s.logistics.truck_convoy.submitted.is_some() || s.logistics.truck_convoy.resolved {
        return Err(illegal("convoy list is already accepted"));
    }
    let orders: Vec<TruckConvoyOrder> = serde_json::from_value(action.clone())
        .map_err(|_| illegal("invalid convoy list; [] is pass"))?;
    validate_plan(c, s, expected.side, &orders, strict)?;
    s.logistics.truck_convoy.submitted = Some(orders);
    Ok("Convoy list recorded; physical effects await closure.".into())
}
fn note(cx: &mut Cx<'_>, side: Side, text: &str) {
    cx.emit(EngineEvent::new(
        Audience::Side(side),
        GameEvent::Note { text: text.into() },
    ));
}
/// Resolve edges and mandatory stops on one State/RNG/event transaction.
/// Cases: airlog:49.18, airlog:52.42, airlog:53.25, land:10.29, land:21.43
pub fn finish(
    c: &CnaContent,
    s: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let mut draft = s.clone();
    let mut rng = cx.rng.clone();
    let mut events = vec![];
    finish_inner(
        c,
        &mut draft,
        strict,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )?;
    *s = draft;
    *cx.rng = rng;
    cx.events.extend(events);
    Ok(())
}
fn finish_inner(
    c: &CnaContent,
    s: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let expected = key(s)?;
    s.logistics
        .truck_convoy
        .validate()
        .map_err(|e| invariant(&e))?;
    if s.logistics.truck_convoy.key.as_ref() != Some(&expected) {
        if s.logistics.truck_convoy.key.is_some() && !s.logistics.truck_convoy.resolved {
            return Err(invariant("unfinished earlier closure"));
        }
        s.logistics.truck_convoy = TruckConvoyProcedure {
            key: Some(expected.clone()),
            ..Default::default()
        };
    }
    if s.logistics.truck_convoy.resolved || !s.decisions.pending.is_empty() {
        return Ok(());
    }
    if let Some(window) = &s.land.breakdown.window.pool {
        if s.logistics.truck_convoy.waiting_pool.as_deref() != Some(window.pool.as_str()) {
            return Err(invariant(
                "restored loss window does not bind the saved convoy wait",
            ));
        }
    } else if let Some(id) = &s.logistics.truck_convoy.waiting_pool {
        // An unrolled stop or its persisted loss window must exist. Missing evidence
        // cannot mean Complete: that would skip a mandatory saved adjudication.
        if s.land
            .breakdown
            .pools
            .get(id)
            .is_none_or(|b| b.stopped.is_empty())
        {
            return Err(invariant(
                "saved convoy wait has neither a stop nor a loss window",
            ));
        }
    }
    if s.logistics.truck_convoy.submitted.is_none() {
        let menu = space(s, expected.side);
        open(
            s,
            cx,
            SeatId::new(expected.side, Role::Logistics),
            KIND,
            "Submit one complete list of whole-pool Move orders, or [].".into(),
            &[
                "airlog:53.12",
                "airlog:53.21",
                "airlog:53.22",
                "airlog:52.42",
            ],
            Trigger::Scheduled,
            Secrecy::Secret,
            menu,
        );
        return Ok(());
    }
    if s.logistics.truck_convoy.next_order == 0 && s.logistics.truck_convoy.waiting_pool.is_none() {
        let orders = s.logistics.truck_convoy.submitted.as_ref().unwrap();
        validate_plan(c, s, expected.side, orders, strict).map_err(accepted)?;
    }
    loop {
        if let Some(id) = s.logistics.truck_convoy.waiting_pool.clone() {
            match breakdown::window::finish_pool(c, s, &id, strict, cx)? {
                breakdown::window::PoolFinish::Waiting => return Ok(()),
                breakdown::window::PoolFinish::Loss { outcome, plan } => {
                    apply_loss(c, s, &outcome, &plan, cx)?;
                    continue;
                }
                breakdown::window::PoolFinish::Complete => {
                    s.logistics.truck_convoy.waiting_pool = None;
                    if !s.decisions.pending.is_empty() {
                        return Ok(());
                    }
                }
            }
        }
        let next = usize::try_from(s.logistics.truck_convoy.next_order)
            .map_err(|_| invariant("next order overflow"))?;
        let Some(order) = s
            .logistics
            .truck_convoy
            .submitted
            .as_ref()
            .and_then(|o| o.get(next))
            .cloned()
        else {
            s.logistics.truck_convoy.submitted = None;
            s.logistics.truck_convoy.next_order = 0;
            s.logistics.truck_convoy.resolved = true;
            return Ok(());
        };
        let (id, path) = order.parts();
        let moved = execute_move(c, s, expected.side, id, path, strict, cx)?;
        s.logistics.truck_convoy.next_order = s
            .logistics
            .truck_convoy
            .next_order
            .checked_add(1)
            .ok_or_else(|| invariant("next order overflow"))?;
        if moved {
            s.logistics.truck_convoy.waiting_pool = Some(id.into());
        }
    }
}
fn execute_move(
    c: &CnaContent,
    s: &mut State,
    side: Side,
    id: &str,
    path: &[HexId],
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<bool, EngineError> {
    if path.is_empty() {
        return Ok(false);
    }
    let p = pool(s, side, id).map_err(accepted)?.clone();
    let mut from = p
        .location
        .as_ref()
        .and_then(Location::hex)
        .cloned()
        .ok_or_else(|| invariant("unresolved trusted pool"))?;
    let cpa = ceiling(c, p.trucks).map_err(accepted)?;
    uniform(s, side, id, p.cargo).map_err(accepted)?;
    let mut moved = false;
    for to in path {
        let cost = match convoy_move::adjudicate_pool_edge(c, s, id, &from, to, strict)? {
            convoy_move::PoolEdge::Blocked => {
                note(
                    cx,
                    side,
                    "Convoy stops before an edge it may not enter; the rest of this order is discarded.",
                );
                break;
            }
            convoy_move::PoolEdge::Pass(cost) => cost,
        };
        let contact = if moved {
            0
        } else {
            convoy_move::pool_departure_cp(c, s, id, strict)?
        };
        let delta = cost
            .cp_quarters
            .checked_add(contact)
            .ok_or_else(|| invariant("edge/contact CP overflow"))?;
        let (groups, prior) = current(s, side, id, cpa).map_err(accepted)?;
        let next = prior
            .spent_cp_quarters
            .checked_add(delta)
            .ok_or_else(|| invariant("physical CP overflow"))?;
        // Lead-approved finish-only resource stop: no debit for this edge,
        // retain the legal prefix and continue later accepted orders.
        if next > cpa {
            note(
                cx,
                side,
                "Convoy stops before travel exceeding its current CPA.",
            );
            break;
        }
        let prior_segment = s
            .logistics
            .pool_fuel_segments
            .get(id)
            .filter(|l| l.segment == segment::SegmentKey::current(s))
            .map_or(0, |l| l.cp_quarters);
        let fuel_cp = prior_segment
            .checked_add(delta)
            .ok_or_else(|| invariant("fuel CP overflow"))?;
        let mut edge = s.clone();
        if let Err(error) = pool_fuel::spend_pool_segment_fuel(c, &mut edge, id, fuel_cp) {
            if error == SupplyError::Insufficient {
                note(cx, side, "Convoy stops before unfunded travel.");
                break;
            }
            return Err(accepted(supply(error)));
        }
        if let Err(error) =
            cargo_history::advance(&mut edge, side, &CargoSite::Pool(id.into()), prior, delta)
        {
            if error == CargoError::Ceiling {
                note(
                    cx,
                    side,
                    "Convoy stops at the retained cargo CPA ceiling (53.25).",
                );
                break;
            }
            return Err(accepted(cargo(error)));
        }
        motion::advance(&mut edge, side, &CargoSite::Pool(id.into()), &groups, delta)
            .map_err(|e| accepted(physical(e)))?;
        if !moved {
            let need = demand(c, s, side, id).map_err(accepted)?;
            breakdown::pools::begin_motion(c, &mut edge, id, &from, strict)?;
            let pool = edge
                .logistics
                .truck_pools
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or_else(|| invariant("pool vanished"))?;
            pool.activity_water = WaterPoints::new(
                pool.activity_water
                    .get()
                    .checked_sub(need)
                    .filter(|n| *n >= 0)
                    .ok_or_else(|| invariant("reserve shortage"))?,
            );
        }
        let local_weather = weather::at_hex(c, &edge, to)?;
        breakdown::pools::record_edge(
            &mut edge,
            id,
            &from,
            cost.breakdown_quarters,
            cost.cp_quarters,
            local_weather,
        )?;
        breakdown::pools::record_light_extra(&mut edge, id, cost.light_extra_quarters)?;
        edge.logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap()
            .location = Some(Location::Hex { hex: to.clone() });
        convoy_move::record_pool_posture(&mut edge, id, cost.on_network)?;
        *s = edge;
        moved = true;
        from = to.clone();
        if cost.assumed_edges {
            note(
                cx,
                side,
                "Convoy travel used a development-profile unassessed edge assumption.",
            );
        }
    }
    if moved {
        if !s
            .land
            .breakdown
            .pools
            .get(id)
            .is_some_and(|b| b.moving.is_some())
        {
            return Err(invariant("actual move has no breakdown motion"));
        }
        breakdown::pools::stop(s, id, &from);
        note(
            cx,
            side,
            "Convoy Move executed; its stop resolves before the next order.",
        );
    }
    Ok(moved)
}
fn equipment(kind: FuelTruckKind) -> breakdown::Equipment {
    match kind {
        FuelTruckKind::Light => breakdown::Equipment::LightTruck,
        FuelTruckKind::Medium => breakdown::Equipment::MediumTruck,
        FuelTruckKind::Heavy => breakdown::Equipment::HeavyTruck,
    }
}
fn truck_counts(groups: &[TruckFuelCohort<String>]) -> Result<Trucks, EngineError> {
    let mut trucks = Trucks::default();
    for g in groups {
        let count = match g.kind {
            FuelTruckKind::Light => &mut trucks.light,
            FuelTruckKind::Medium => &mut trucks.medium,
            FuelTruckKind::Heavy => &mut trucks.heavy,
        };
        *count = count
            .checked_add(g.count)
            .filter(|n| *n >= 0)
            .ok_or_else(|| invariant("loss count overflow"))?;
    }
    Ok(trucks)
}
fn marker(
    side: Side,
    pool: &str,
    hex: HexId,
    groups: Vec<TruckFuelCohort<String>>,
    packing: CargoPacking,
    tank: i32,
    water: i32,
) -> breakdown::markers::BrokenMarker {
    breakdown::markers::BrokenMarker {
        id: String::new(),
        side,
        hex,
        assets: vec![],
        source_pool: Some(pool.into()),
        pool_assets: groups
            .iter()
            .map(|g| breakdown::pools::PoolAsset {
                pool: pool.into(),
                equipment: equipment(g.kind),
                points: g.count,
                cohort: g.id.clone(),
            })
            .collect(),
        pool_fuel_cohorts: groups,
        passengers: BTreeMap::new(),
        transport: Trucks::default(),
        cargo: packing,
        tank_fuel: FuelTenths::new(tank),
        activity_water: WaterPoints::new(water),
        fuel_cohorts: vec![],
        paid_truck_water: Default::default(),
        water_credit_stage: None,
    }
}
fn apply_loss(
    c: &CnaContent,
    s: &mut State,
    o: &breakdown::pool_losses::PoolOutcome,
    p: &breakdown::pool_losses::PoolLossPlan,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    breakdown::pool_losses::validate(c, s, o, p).map_err(accepted)?;
    let before = pool(s, o.group.side, &p.pool).map_err(accepted)?.clone();
    let full = pool_fuel::pool_segment_fuel_cohorts(s, &p.pool).map_err(|e| accepted(supply(e)))?;
    let physicals: Vec<_> = full.iter().map(motion::PhysicalTrucks::from).collect();
    motion::query(s, before.side, &CargoSite::Pool(p.pool.clone()), &physicals)
        .map_err(|_| invariant("full loss motion is corrupt"))?;
    let mut ledger = s
        .logistics
        .pool_fuel_segments
        .get(&p.pool)
        .cloned()
        .ok_or_else(|| invariant("actual moved pool has no fuel ledger"))?;
    let origins = segment::select_named(&mut ledger, &format!("pool.{}", p.pool), &p.at_origin)
        .map_err(|e| accepted(supply(e)))?;
    let at: BTreeMap<_, _> = p
        .at_origin
        .iter()
        .map(|a| (a.id.as_str(), a.count))
        .collect();
    let destinations: Vec<_> = p
        .losses
        .iter()
        .filter_map(|loss| {
            let remaining = loss.count - at.get(loss.id.as_str()).copied().unwrap_or(0);
            (remaining > 0).then(|| FuelCohortSelection {
                id: loss.id.clone(),
                count: remaining,
            })
        })
        .collect();
    let destinations =
        segment::select_named(&mut ledger, &format!("pool.{}", p.pool), &destinations)
            .map_err(|e| accepted(supply(e)))?;
    let working = truck_counts(&ledger.cohorts)?;
    let origin_counts = truck_counts(&origins)?;
    let destination_counts = truck_counts(&destinations)?;
    for (initial, retained, origin, destination) in [
        (
            before.trucks.light,
            working.light,
            origin_counts.light,
            destination_counts.light,
        ),
        (
            before.trucks.medium,
            working.medium,
            origin_counts.medium,
            destination_counts.medium,
        ),
        (
            before.trucks.heavy,
            working.heavy,
            origin_counts.heavy,
            destination_counts.heavy,
        ),
    ] {
        if retained
            .checked_add(origin)
            .and_then(|n| n.checked_add(destination))
            != Some(initial)
        {
            return Err(invariant("selected equipment partition is not conserved"));
        }
    }
    let q = &p.partition;
    let candidates = [
        marker(
            before.side,
            &p.pool,
            o.origin.clone(),
            origins,
            q.origin.clone(),
            q.origin_tank_fuel_tenths,
            q.origin_activity_water,
        ),
        marker(
            before.side,
            &p.pool,
            o.destination.clone(),
            destinations,
            q.destination.clone(),
            q.destination_tank_fuel_tenths,
            q.destination_activity_water,
        ),
    ];
    let prepared = breakdown::markers::prepare_pool_markers(c, s, &candidates)?;
    let old_ids: BTreeSet<_> = s.land.breakdown.markers.keys().cloned().collect();
    let events = breakdown::markers::apply_pool_markers(c, s, prepared)?;
    let new: Vec<_> = s
        .land
        .breakdown
        .markers
        .values()
        .filter(|m| !old_ids.contains(&m.id))
        .cloned()
        .collect();
    if new.len()
        != candidates
            .iter()
            .filter(|m| !m.pool_fuel_cohorts.is_empty())
            .count()
    {
        return Err(invariant("marker batch footprint mismatch"));
    }
    let mut shares = vec![];
    for m in &new {
        if m.side != before.side
            || m.source_pool.as_deref() != Some(p.pool.as_str())
            || !candidates.iter().any(|candidate| {
                candidate.hex == m.hex
                    && candidate.pool_fuel_cohorts == m.pool_fuel_cohorts
                    && candidate.pool_assets == m.pool_assets
                    && candidate.cargo == m.cargo
                    && candidate.tank_fuel == m.tank_fuel
                    && candidate.activity_water == m.activity_water
            })
        {
            return Err(invariant(
                "actual marker does not match exact selected consequence",
            ));
        }
        shares.push(cargo_history::relocation::PoolCargoShare {
            marker: m.id.clone(),
            goods: m.cargo.totals().map_err(|e| accepted(supply(e)))?,
        });
    }
    let relocation = cargo_history::relocation::prepare_pool_breakdown_relocation(
        c,
        s,
        before.side,
        &p.pool,
        &q.working,
        &shares,
    )?;
    // Actual markers already exist; selected returned ids/parents transfer from the one
    // pre-loss current footprint. Neither origin nor destination receives fresh CP.
    for m in &new {
        let groups: Vec<_> = m
            .pool_fuel_cohorts
            .iter()
            .map(motion::PhysicalTrucks::from)
            .collect();
        motion::transfer(
            s,
            before.side,
            &CargoSite::Pool(p.pool.clone()),
            &CargoSite::BrokenMarker(m.id.clone()),
            &groups,
        )
        .map_err(|_| invariant("selected loss physical transfer failed"))?;
    }
    s.logistics
        .pool_fuel_segments
        .insert(p.pool.clone(), ledger);
    let pool = s
        .logistics
        .truck_pools
        .iter_mut()
        .find(|pool| pool.id == p.pool)
        .ok_or_else(|| invariant("loss pool vanished"))?;
    pool.trucks = working;
    pool.cargo = q.working.totals().map_err(|e| accepted(supply(e)))?;
    pool.tank_fuel = FuelTenths::new(
        before
            .tank_fuel
            .get()
            .checked_sub(q.origin_tank_fuel_tenths)
            .and_then(|n| n.checked_sub(q.destination_tank_fuel_tenths))
            .filter(|n| *n >= 0)
            .ok_or_else(|| invariant("loss tank partition underflow"))?,
    );
    pool.activity_water = WaterPoints::new(
        before
            .activity_water
            .get()
            .checked_sub(q.origin_activity_water)
            .and_then(|n| n.checked_sub(q.destination_activity_water))
            .filter(|n| *n >= 0)
            .ok_or_else(|| invariant("loss water partition underflow"))?,
    );
    if working == Trucks::default() {
        s.land.movement.pool_on_road.remove(&p.pool);
    }
    cargo_history::relocation::apply_pool_breakdown_relocation(s, relocation)?;
    cx.events.extend(events);
    Ok(())
}
/// Deterministic controller pass; movement intent remains the owning answer's choice.
pub fn baseline() -> Value {
    json!([])
}
/// Private query registration uses an already authorized own pool inspection.
/// Cases: airlog:53.22, airlog:53.25, land:3.6
pub fn own_report(c: &CnaContent, s: &State, side: Side, id: &str) -> Result<Value, Rejection> {
    let p = pool(s, side, id)?;
    if p.location.as_ref().and_then(Location::hex).is_none() || p.trucks == Trucks::default() {
        return Ok(json!({"kind":KIND,"eligible":false,
            "reason":"this slice requires a resolved pool with truck points"}));
    }
    let cpa = ceiling(c, p.trucks)?;
    let cohorts = pool_fuel::pool_segment_fuel_cohorts(s, id).map_err(supply)?;
    let groups: Vec<_> = cohorts.iter().map(motion::PhysicalTrucks::from).collect();
    let site = CargoSite::Pool(id.into());
    let mut report = json!({"kind":KIND,"ceiling_cp_quarters":cpa,"physical":groups,
        "pool_on_road":s.land.movement.pool_on_road.contains(id),
        "rules":["airlog:53.22","airlog:53.25","land:9.33"]});
    match motion::timing(s, side, &site, &groups, cpa) {
        Ok(timing) => {
            report["timing"] = json!("known");
            report["spent_cp_quarters"] = json!(timing.spent_cp_quarters);
        }
        Err(motion::MotionError::Unknown) => {
            if s.logistics
                .cargo_history
                .motion
                .entries
                .iter()
                .any(|e| e.site == site && e.stage == super::water::WaterStage::current(s))
            {
                return Err(Rejection::Engine(invariant(
                    "owner report current physical footprint is corrupt",
                )));
            }
            report["timing"] = json!("unknown");
        }
        Err(motion::MotionError::Mixed) => report["timing"] = json!("mixed"),
        Err(motion::MotionError::Invalid) => {
            return Err(Rejection::Engine(invariant(
                "owner report physical timing is corrupt",
            )));
        }
    }
    Ok(report)
}

#[cfg(test)]
#[path = "truck_convoy/tests.rs"]
mod tests;
