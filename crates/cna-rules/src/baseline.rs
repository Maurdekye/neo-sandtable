//! Scripted movement choices use only own reachability and public enemy stack positions.
//! Pass a controller-local RNG, never the campaign's adjudication RNG.
use crate::{
    CnaContent, State,
    land::{movement, zoc},
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
/// Under full, avoid public enemy adjacency because reaction is not supported yet.
/// Cases: land:8.11, land:8.13, land:10.6, land:19.44
pub fn random_orders(
    content: &CnaContent,
    state: &State,
    request: &DecisionRequest,
    rng: &mut CampaignRng,
) -> Value {
    if request.kind != movement::KIND {
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
        let paths: Vec<_> = movement::reachable(content, state, &id, strict)
            .into_iter()
            .filter(|r| {
                !r.path.is_empty()
                    && (!strict
                        || r.path.iter().all(|hex| {
                            !zoc::possibly_controlled(
                                content,
                                state,
                                request.seat.side.opponent(),
                                hex,
                            )
                        }))
            })
            .collect();
        if paths.is_empty() {
            continue;
        }
        let path = &paths[index(rng, paths.len())];
        return json!([{"unit":id,"path":path.path}]);
    }
    json!([])
}
