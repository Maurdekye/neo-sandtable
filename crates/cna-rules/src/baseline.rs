//! Scripted movement choices use only own reachability and public enemy stack positions.
//! Pass a controller-local RNG, never the campaign's adjudication RNG.
pub use crate::logistics::baseline::logistics_orders;
use crate::{
    CnaContent, State,
    land::{movement, reaction},
};
use cna_core::{
    decision::{ActionSchema, DecisionRequest},
    dice::CampaignRng,
};
use serde_json::{Value, json};

fn index(rng: &mut CampaignRng, count: usize) -> usize {
    let count = count as u128;
    let mut space = 1u128;
    let mut digits = 0;
    while space < count {
        space *= 6;
        digits += 1;
    }
    let limit = space - space % count;
    loop {
        let mut value = 0u128;
        for _ in 0..digits {
            value = value * 6 + u128::from(rng.d6().value() - 1);
        }
        if value < limit {
            return (value % count) as usize;
        }
    }
}
/// Choose at most one complete move, preventing competing draws/stack destinations in a list.
/// Uncertain control may truncate the move at execution but is never queried by this controller.
/// Reaction and continuation use the same own-information search.
/// Cases: land:8.11, land:8.13, land:10.6, land:19.44
pub fn random_orders(
    content: &CnaContent,
    state: &State,
    request: &DecisionRequest,
    rng: &mut CampaignRng,
) -> Value {
    if !matches!(
        request.kind.as_str(),
        movement::KIND | reaction::KIND | reaction::CONTINUE
    ) {
        return Value::Null;
    }
    let ActionSchema::List { item, .. } = &request.space.schema else {
        return json!([]);
    };
    let ActionSchema::Record { fields } = item.as_ref() else {
        return json!([]);
    };
    let Some(ActionSchema::Unit { among }) =
        fields.iter().find(|f| f.name == "unit").map(|f| &f.schema)
    else {
        return json!([]);
    };
    let mut units = among.clone();
    while !units.is_empty() {
        let id = units.remove(index(rng, units.len()));
        if !state
            .land
            .units
            .get(&id)
            .is_some_and(|u| u.side == request.seat.side)
        {
            continue;
        }
        let strict = state.land.movement.strict;
        if request.kind == reaction::KIND {
            let options = reaction::plans(content, state, &id, strict);
            if options.is_empty() {
                continue;
            }
            let (division, paths) = &options[index(rng, options.len())];
            let path = &paths[index(rng, paths.len())];
            return json!([{"unit":id,"path":path.path,"truck_division":division}]);
        }
        let paths: Vec<_> = movement::reachable(content, state, &id, strict)
            .into_iter()
            .filter(|r| !r.path.is_empty())
            .collect();
        if paths.is_empty() {
            continue;
        }
        let path = &paths[index(rng, paths.len())];
        return json!([{"unit":id,"path":path.path}]);
    }
    json!([])
}

pub use crate::land::combat::random_positions;

pub use crate::land::combat::barrage::random_plans as random_barrages;

pub use crate::land::combat::retreat::random_orders as random_retreats;

/// Provide an exact conserved own breakdown allocation after the roll has been disclosed.
/// Cases: land:21.35, land:21.36, land:21.41, land:21.43
pub fn random_breakdown(
    content: &CnaContent,
    state: &State,
    request: &DecisionRequest,
    _rng: &mut CampaignRng,
) -> Value {
    if request.kind != crate::land::breakdown::window::KIND {
        return Value::Null;
    }
    if let Some(w) = &state.land.breakdown.window.pool {
        return w
            .outcomes
            .front()
            .filter(|o| {
                o.group.side == request.seat.side
                    && request.seat.role == cna_protocol::Role::Logistics
            })
            .and_then(|o| crate::land::breakdown::pool_losses::plan(content, state, o))
            .and_then(|p| serde_json::to_value(p).ok())
            .unwrap_or(Value::Null);
    }
    state
        .land
        .breakdown
        .window
        .outcomes
        .front()
        .filter(|o| {
            o.group.assets.first().is_some_and(|a| {
                state.land.units[&a.unit].side == request.seat.side
                    && crate::ownership::seat_for_unit(content, state, &a.unit) == request.seat.role
            })
        })
        .and_then(|o| crate::land::breakdown::baseline::plan(content, state, o))
        .and_then(|p| serde_json::to_value(p).ok())
        .unwrap_or(Value::Null)
}
pub use crate::land::combat::assignment::random_orders as random_assignments;

/// Source-conserving fixed arrival/withdrawal role plans.
pub use crate::land::arrivals::arrival_orders;
