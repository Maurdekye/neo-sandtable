//! Same-location supply transfers. Truck cargo is unloaded before another truck loads it.
//! This procedure handles the free Supply Distribution window only. Other windows must
//! charge loading CP and preserve the no-leapfrogging cargo ledger before using this API.
use super::stores::{engine, field, option};
use super::{CargoPacking, SupplyError, capacity, fuel_capacity, validate_packing};
use crate::{
    CnaContent,
    state::{DumpLocation, Location, Pending, State},
    steps::{illegal, open},
};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_core::{
    decision::{ActionSchema, ActionSpace, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{SeatId, UnitId},
    quantity::FuelTenths,
    visibility::Audience,
};
use cna_protocol::{GameEvent, Side};
use cna_tables::airlog::supply::{DumpCapacity, DumpLocation as ChartLocation, SupplyType};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const KIND: &str = "cna.logistics.distribution";
pub const PREFIX: &str = "cna.logistics.distribution.to:";
const TYPES: [SupplyType; 4] = [
    SupplyType::Ammo,
    SupplyType::Fuel,
    SupplyType::Stores,
    SupplyType::Water,
];

/// Stable owner-only transfer identities; tanks are receivers, never ordinary sources.
/// Cases: airlog:49.16, airlog:49.17, airlog:50.15, airlog:53.24
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Endpoint {
    Dump(String),
    Cargo(UnitId),
    Pool(String),
    Tank(UnitId),
    Ground(Location),
}

/// Cases: airlog:53.24, airlog:54.11
pub fn location(state: &State, side: Side, end: &Endpoint) -> Result<Location, SupplyError> {
    match end {
        Endpoint::Ground(at) => {
            if matches!(at, Location::Hex { .. })
                && (state
                    .land
                    .units
                    .values()
                    .any(|u| u.side == side && &u.location == at)
                    || state
                        .logistics
                        .truck_pools
                        .iter()
                        .any(|p| p.side == side && p.location.as_ref() == Some(at)))
            {
                Ok(at.clone())
            } else {
                Err(SupplyError::Invalid)
            }
        }
        Endpoint::Dump(id) => {
            let d = state
                .logistics
                .dumps
                .get(id)
                .filter(|d| d.side == side && d.active && !d.dummy)
                .ok_or(SupplyError::Invalid)?;
            match &d.location {
                DumpLocation::Hex { hex } => Ok(Location::Hex { hex: hex.clone() }),
                DumpLocation::OffMap { id } => Ok(Location::OffMap { id: id.clone() }),
                _ => Err(SupplyError::Invalid),
            }
        }
        Endpoint::Cargo(id) | Endpoint::Tank(id) => state
            .land
            .units
            .get(id)
            .filter(|u| {
                u.side == side
                    && matches!(u.location, Location::Hex { .. } | Location::OffMap { .. })
            })
            .map(|u| u.location.clone())
            .ok_or(SupplyError::Invalid),
        Endpoint::Pool(id) => state
            .logistics
            .truck_pools
            .iter()
            .find(|p| &p.id == id && p.side == side)
            .and_then(|p| p.location.clone())
            .filter(|l| matches!(l, Location::Hex { .. } | Location::OffMap { .. }))
            .ok_or(SupplyError::Invalid),
    }
}
pub(super) fn stock(state: &State, end: &Endpoint) -> Result<Supplies, SupplyError> {
    Ok(match end {
        Endpoint::Dump(id) => {
            state
                .logistics
                .dumps
                .get(id)
                .ok_or(SupplyError::Invalid)?
                .supplies
        }
        Endpoint::Cargo(id) => state
            .logistics
            .unit_supply
            .get(id)
            .map_or(Supplies::default(), |h| h.carried),
        Endpoint::Pool(id) => {
            state
                .logistics
                .truck_pools
                .iter()
                .find(|p| &p.id == id)
                .ok_or(SupplyError::Invalid)?
                .cargo
        }
        Endpoint::Tank(_) | Endpoint::Ground(_) => return Err(SupplyError::Invalid),
    })
}
fn set_stock(state: &mut State, end: &Endpoint, s: Supplies) {
    match end {
        Endpoint::Dump(id) => state.logistics.dumps.get_mut(id).unwrap().supplies = s,
        Endpoint::Cargo(id) => {
            state
                .logistics
                .unit_supply
                .entry(id.clone())
                .or_default()
                .carried = s
        }
        Endpoint::Pool(id) => {
            state
                .logistics
                .truck_pools
                .iter_mut()
                .find(|p| &p.id == id)
                .unwrap()
                .cargo = s
        }
        Endpoint::Tank(_) | Endpoint::Ground(_) => unreachable!(),
    }
}
fn truck(end: &Endpoint) -> bool {
    matches!(end, Endpoint::Cargo(_) | Endpoint::Pool(_))
}
pub(super) fn endpoints(state: &State, side: Side) -> Vec<Endpoint> {
    let mut out = Vec::new();
    out.extend(
        state
            .logistics
            .dumps
            .values()
            .filter(|d| d.side == side && d.active && !d.dummy)
            .map(|d| Endpoint::Dump(d.id.clone())),
    );
    for u in state.land.units.values().filter(|u| u.side == side) {
        if u.trucks.light + u.trucks.medium + u.trucks.heavy > 0 {
            out.push(Endpoint::Cargo(u.id.clone()));
        }
        out.push(Endpoint::Tank(u.id.clone()));
    }
    out.extend(
        state
            .logistics
            .truck_pools
            .iter()
            .filter(|p| p.side == side)
            .map(|p| Endpoint::Pool(p.id.clone())),
    );
    let grounds: Vec<_> = out
        .iter()
        .filter_map(|e| location(state, side, e).ok())
        .filter(|at| matches!(at, Location::Hex { .. }))
        .collect();
    for at in grounds {
        let end = Endpoint::Ground(at);
        if !out.contains(&end) {
            out.push(end)
        }
    }
    out.into_iter()
        .filter(|e| location(state, side, e).is_ok())
        .collect()
}
pub(super) fn sources(state: &State, side: Side, to: &Endpoint) -> Vec<Endpoint> {
    let Ok(at) = location(state, side, to) else {
        return vec![];
    };
    endpoints(state, side)
        .into_iter()
        .filter(|e| {
            e != to
                && !matches!(e, Endpoint::Tank(_) | Endpoint::Ground(_))
                && !(truck(e) && truck(to))
                && !(matches!(e, Endpoint::Pool(_)) && matches!(to, Endpoint::Tank(_)))
                && location(state, side, e).is_ok_and(|l| l == at)
                && stock(state, e).is_ok_and(|s| {
                    if matches!(to, Endpoint::Tank(_)) {
                        s.fuel > 0
                    } else {
                        TYPES.into_iter().any(|t| capacity::points(&s, t) > 0)
                    }
                })
        })
        .collect()
}

/// Aggregate all real friendly dumps at this location, so extra counters cannot expand
/// hex capacity. With incomplete settlement data only a bound valid for every possible
/// dump classification is accepted; increases above it require verified membership.
/// Cases: airlog:54.11, airlog:54.13
pub fn validate_dump_capacity(
    content: &CnaContent,
    state: &State,
    side: Side,
    at: &Location,
    addition: &Supplies,
) -> Result<(), SupplyError> {
    let mut total = Supplies::default();
    for d in state
        .logistics
        .dumps
        .values()
        .filter(|d| d.side == side && d.active && !d.dummy)
    {
        let loc = match &d.location {
            DumpLocation::Hex { hex } => Location::Hex { hex: hex.clone() },
            DumpLocation::OffMap { id } => Location::OffMap { id: id.clone() },
            _ => continue,
        };
        if &loc == at {
            for t in TYPES {
                let n = capacity::points(&total, t)
                    .checked_add(capacity::points(&d.supplies, t))
                    .ok_or(SupplyError::Invalid)?;
                capacity::set_points(&mut total, t, n);
            }
        }
    }
    let class = match at {
        Location::OffMap { id }
            if ["box_tripoli", "box_tripolitania", "box_tunis", "box_gabes"]
                .contains(&id.as_str()) =>
        {
            Some(ChartLocation::TunisTripoli)
        }
        Location::Hex { hex } if content.places.at(hex).any(|p| p.kind == "major_city") => {
            Some(ChartLocation::MajorCity)
        }
        Location::Hex { hex }
            if content
                .places
                .at(hex)
                .any(|p| p.kind == "village" || p.kind == "town") =>
        {
            Some(ChartLocation::Village)
        }
        Location::Hex { .. } => None,
        _ => {
            return Err(SupplyError::Unsupported {
                case: "airlog:54.13",
            });
        }
    };
    for t in TYPES {
        let n = capacity::points(addition, t);
        if n < 0 {
            return Err(SupplyError::Invalid);
        }
        if n == 0 {
            continue;
        }
        let sum = capacity::points(&total, t)
            .checked_add(n)
            .ok_or(SupplyError::Invalid)?;
        let cap = content
            .tables
            .airlog
            .supply_dump_capacity
            .capacity(class.unwrap_or(ChartLocation::OtherTerrain), t);
        if matches!(cap,DumpCapacity::Max(max) if sum>max) {
            return Err(if class.is_none() {
                SupplyError::Unsupported {
                    case: "airlog:54.13",
                }
            } else {
                SupplyError::Insufficient
            });
        }
    }
    Ok(())
}

/// Atomically load/unload one friendly same-location stock. For a tank receiver amount.fuel
/// is in tenths, and the whole-point cargo source pays the ceiling exactly once. For all
/// other receivers every amount is in whole points. Pools must unload to a dump first.
/// Cases: airlog:49.14, airlog:49.16, airlog:50.15, airlog:53.24, airlog:54.13, airlog:54.2
/// Interpretations: interp:airlog-0001, interp:airlog-0008
#[allow(clippy::too_many_arguments)]
pub fn transfer(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    from: &Endpoint,
    to: &Endpoint,
    amount: Supplies,
    packing: &CargoPacking,
) -> Result<(), SupplyError> {
    if state.cursor.anchor() != "opstage.organization.supply_distribution" || from == to {
        return Err(SupplyError::Invalid);
    }
    let at = location(state, side, from)?;
    if at != location(state, side, to)?
        || (truck(from) && truck(to))
        || (matches!(from, Endpoint::Pool(_)) && matches!(to, Endpoint::Tank(_)))
    {
        return Err(SupplyError::Invalid);
    }
    if TYPES.into_iter().any(|t| capacity::points(&amount, t) < 0)
        || TYPES.into_iter().all(|t| capacity::points(&amount, t) == 0)
    {
        return Err(SupplyError::Invalid);
    }
    let mut debit = amount;
    let mut draft = state.clone();
    let target = if let Endpoint::Ground(at) = to {
        let mut n = 1u64;
        let id = loop {
            let id = format!("{}.dump-{n}", crate::state::side_key(side));
            if !draft.logistics.dumps.contains_key(&id) {
                break id;
            }
            n = n.checked_add(1).ok_or(SupplyError::Invalid)?;
        };
        let Location::Hex { hex } = at else {
            return Err(SupplyError::Invalid);
        };
        let marker = super::dump_markers::next_marker(&mut draft.logistics)?;
        draft.logistics.dumps.insert(
            id.clone(),
            crate::state::Dump {
                marker,
                id: id.clone(),
                side,
                location: DumpLocation::Hex { hex: hex.clone() },
                supplies: Supplies::default(),
                active: true,
                dummy: false,
            },
        );
        Endpoint::Dump(id)
    } else {
        to.clone()
    };
    let to = &target;
    if let Endpoint::Tank(id) = to {
        if amount.ammo != 0 || amount.stores != 0 || amount.water != 0 {
            return Err(SupplyError::Invalid);
        }
        let cap = fuel_capacity(content, &draft, id)?;
        let holding = draft.logistics.unit_supply.entry(id.clone()).or_default();
        let new = holding
            .tank_fuel
            .get()
            .checked_add(amount.fuel)
            .ok_or(SupplyError::Invalid)?;
        if new > cap.get() {
            return Err(SupplyError::Insufficient);
        }
        holding.tank_fuel = FuelTenths::new(new);
        debit.fuel =
            i32::try_from((i64::from(amount.fuel) + 9) / 10).map_err(|_| SupplyError::Invalid)?;
    } else {
        let mut target = stock(&draft, to)?;
        for t in TYPES {
            let n = capacity::points(&target, t)
                .checked_add(capacity::points(&amount, t))
                .ok_or(SupplyError::Invalid)?;
            capacity::set_points(&mut target, t, n);
        }
        match to {
            Endpoint::Dump(_) => validate_dump_capacity(
                content,
                state,
                side,
                &at,
                &if matches!(from, Endpoint::Dump(_)) {
                    Supplies::default()
                } else {
                    amount
                },
            )?,
            Endpoint::Cargo(id) => {
                let u = &state.land.units[id];
                validate_packing(content, &u.trucks, &u.transport_trucks, &target, packing)?;
            }
            Endpoint::Pool(id) => {
                let p = state
                    .logistics
                    .truck_pools
                    .iter()
                    .find(|p| &p.id == id)
                    .unwrap();
                validate_packing(content, &p.trucks, &Trucks::default(), &target, packing)?;
            }
            Endpoint::Tank(_) | Endpoint::Ground(_) => unreachable!(),
        }
        set_stock(&mut draft, to, target);
    }
    let mut source = stock(&draft, from)?;
    for t in TYPES {
        let n = capacity::points(&source, t)
            .checked_sub(capacity::points(&debit, t))
            .ok_or(SupplyError::Invalid)?;
        if n < 0 {
            return Err(SupplyError::Insufficient);
        }
        capacity::set_points(&mut source, t, n);
    }
    set_stock(&mut draft, from, source);
    state.logistics = draft.logistics;
    Ok(())
}

/// Cases: airlog:48.0, airlog:53.24, land:3.6
pub fn enter(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    for side in [Side::Axis, Side::Commonwealth] {
        menu(content, state, side, cx)?;
    }
    Ok(())
}
fn menu(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let mut choices = vec![option("done".into(), "Finish supply redistribution".into())];
    choices.extend(
        endpoints(state, side)
            .into_iter()
            .filter(|e| match e {
                Endpoint::Tank(id) => fuel_capacity(content, state, id).is_ok_and(|cap| {
                    state
                        .logistics
                        .unit_supply
                        .get(id)
                        .map_or(0, |h| h.tank_fuel.get())
                        < cap.get()
                }),
                _ => true,
            })
            .filter(|e| !sources(state, side, e).is_empty())
            .map(|e| option(serde_json::to_string(&e).unwrap(), format!("Load {e:?}"))),
    );
    if choices.len() == 1 {
        return Ok(());
    }
    open(
        state,
        cx,
        SeatId::new(side, cna_protocol::Role::Logistics),
        KIND,
        "Redistribute friendly supplies in the same location; unload before loading another truck."
            .into(),
        &["airlog:53.24", "airlog:54.13", "land:3.6"],
        Trigger::Scheduled,
        Secrecy::Secret,
        ActionSpace::new(ActionSchema::Choice { options: choices }).with_pass("Finish"),
    );
    Ok(())
}
pub(super) fn packing_schema(content: &CnaContent, state: &State, to: &Endpoint) -> ActionSchema {
    let attached = match to {
        Endpoint::Cargo(id) => state.land.units[id].trucks,
        Endpoint::Pool(id) => {
            state
                .logistics
                .truck_pools
                .iter()
                .find(|p| &p.id == id)
                .unwrap()
                .trucks
        }
        _ => Trucks::default(),
    };
    ActionSchema::Record {
        fields: [
            ("light", cna_tables::airlog::trucks::TruckType::Light),
            ("medium", cna_tables::airlog::trucks::TruckType::Medium),
            ("heavy", cna_tables::airlog::trucks::TruckType::Heavy),
        ]
        .into_iter()
        .map(|(name, t)| {
            field(
                name,
                "Final cargo packing",
                ActionSchema::Record {
                    fields: TYPES
                        .into_iter()
                        .zip(["ammo", "fuel", "stores", "water"])
                        .map(|(s, name)| {
                            field(
                                name,
                                "Whole supply points",
                                ActionSchema::Integer {
                                    min: 0,
                                    max: i64::from(capacity::trucks(&attached, t))
                                        * i64::from(
                                            content
                                                .tables
                                                .airlog
                                                .truck_characteristics
                                                .truck(t)
                                                .supply_capacity(s),
                                        ),
                                },
                            )
                        })
                        .collect(),
                },
            )
        })
        .collect(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Order {
    source: String,
    amount: Supplies,
    packing: CargoPacking,
}
/// Cases: airlog:49.14, airlog:49.16, airlog:53.24, airlog:54.13, land:3.6
/// Interpretations: interp:airlog-0001, interp:airlog-0008
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let side = pending.seat.side;
    if action.is_null() {
        if pending.kind != KIND {
            menu(content, state, side, cx).map_err(Rejection::Engine)?;
        }
        return Ok("Finished redistribution".into());
    }
    if pending.kind == KIND {
        if action.as_str() == Some("done") {
            return Ok("Finished redistribution".into());
        }
        let to: Endpoint = serde_json::from_str(
            action
                .as_str()
                .ok_or_else(|| illegal("select a receiver"))?,
        )
        .map_err(|_| illegal("unknown receiver"))?;
        let sources = sources(state, side, &to);
        if sources.is_empty() {
            return Err(illegal("receiver has no accessible source"));
        }
        let max = |t| {
            sources
                .iter()
                .filter_map(|e| stock(state, e).ok())
                .map(|s| i64::from(capacity::points(&s, t)))
                .max()
                .unwrap_or(0)
        };
        let amount = ActionSchema::Record {
            fields: TYPES
                .into_iter()
                .zip(["ammo", "fuel", "stores", "water"])
                .map(|(t, name)| {
                    field(
                        name,
                        if matches!(to, Endpoint::Tank(_)) && t == SupplyType::Fuel {
                            "Fuel tenths to fill (cargo pays whole-point ceiling)"
                        } else {
                            "Whole points to transfer"
                        },
                        ActionSchema::Integer {
                            min: 0,
                            max: if matches!(to, Endpoint::Tank(_)) {
                                if t == SupplyType::Fuel {
                                    max(t) * 10
                                } else {
                                    0
                                }
                            } else {
                                max(t)
                            },
                        },
                    )
                })
                .collect(),
        };
        open(
            state,
            cx,
            pending.seat,
            &format!("{PREFIX}{}", serde_json::to_string(&to).unwrap()),
            format!("Load {to:?}; choose one friendly source and final packing."),
            &["airlog:53.24", "airlog:54.2", "land:3.6"],
            Trigger::Scheduled,
            Secrecy::Secret,
            ActionSpace::new(ActionSchema::Record {
                fields: vec![
                    field(
                        "source",
                        "Same-location source",
                        ActionSchema::Choice {
                            options: sources
                                .into_iter()
                                .map(|e| {
                                    option(serde_json::to_string(&e).unwrap(), format!("{e:?}"))
                                })
                                .collect(),
                        },
                    ),
                    field("amount", "Supply transfer", amount),
                    field(
                        "packing",
                        "Final cargo by truck type (zero for dumps/tanks)",
                        packing_schema(content, state, &to),
                    ),
                ],
            })
            .with_pass("Return without transferring"),
        );
        return Ok("Selected receiver".into());
    }
    let to: Endpoint = serde_json::from_str(
        pending
            .kind
            .strip_prefix(PREFIX)
            .ok_or_else(|| illegal("unknown transfer"))?,
    )
    .map_err(|_| illegal("unknown receiver"))?;
    let order: Order =
        serde_json::from_value(action.clone()).map_err(|_| illegal("invalid transfer"))?;
    let from: Endpoint =
        serde_json::from_str(&order.source).map_err(|_| illegal("unknown source"))?;
    transfer(
        content,
        state,
        side,
        &from,
        &to,
        order.amount,
        &order.packing,
    )
    .map_err(|e| match e {
        SupplyError::Unsupported { .. } | SupplyError::UnknownFuelRate => {
            Rejection::Engine(engine(e))
        }
        _ => illegal("transfer exceeds available stock, capacity or same-location loading rules"),
    })?;
    cx.emit(EngineEvent::new(
        Audience::Side(side),
        GameEvent::Note {
            text: format!("Transferred {:?} from {from:?} to {to:?}.", order.amount),
        },
    ));
    menu(content, state, side, cx).map_err(Rejection::Engine)?;
    Ok("Supplies redistributed".into())
}

#[cfg(test)]
mod tests;
