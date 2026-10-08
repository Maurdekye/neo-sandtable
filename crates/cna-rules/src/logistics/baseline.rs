//! Controller-local logistics choices. Truth about enemy stocks and undiscovered wells
//! never enters this policy; only the owning side's inventories and known conditions do.
use super::{
    CargoPacking, SupplyDraw, SupplySource, attrition, available_sources_with_content, capacity,
    distribution, movement_restrictions, rations, stores, water, wells,
};
use crate::{CnaContent, State};
use cna_content::scenario::Supplies;
use cna_core::{
    decision::{ActionSchema, DecisionRequest},
    dice::CampaignRng,
    ids::UnitId,
    visibility::Perspective,
};
use serde_json::{Value, json};

fn sources(content: &CnaContent, state: &State, id: &UnitId) -> Vec<SupplyDraw> {
    let mut sources = available_sources_with_content(content, state, id).unwrap_or_default();
    sources.sort_by_key(|d| match d.source {
        SupplySource::Unlimited => 0,
        SupplySource::Dump(_) => 1,
        SupplySource::UnitStock(_) => 2,
        _ => 3,
    });
    sources
}
fn available(sources: &[SupplyDraw], stores: bool) -> i64 {
    sources
        .iter()
        .map(|s| {
            i64::from(if stores {
                s.amount.stores.get()
            } else {
                s.amount.water.get()
            })
        })
        .sum()
}
fn draws(sources: &[SupplyDraw], mut stores: i32, mut water: i32) -> Value {
    let mut out = Vec::new();
    for s in sources {
        let food = stores.min(s.amount.stores.get().max(0));
        let drink = water.min(s.amount.water.get().max(0));
        if food > 0 || drink > 0 {
            out.push(json!({"source":serde_json::to_string(&s.source).unwrap(),"stores":food,"water":drink}));
            stores -= food;
            water -= drink;
        }
    }
    json!(out)
}
fn priority(content: &CnaContent, state: &State, id: &UnitId) -> u8 {
    match movement_restrictions(content, state, id) {
        Ok(r) if !r.may_move || !r.may_exceed_cpa || !r.may_offensive_close_assault => 0,
        _ => 1,
    }
}
fn amount(request: &DecisionRequest, name: &str) -> i32 {
    let ActionSchema::Record { fields } = &request.space.schema else {
        return 0;
    };
    fields
        .iter()
        .find(|f| f.name == name)
        .and_then(|f| match f.schema {
            ActionSchema::Integer { max, .. } => i32::try_from(max).ok(),
            _ => None,
        })
        .unwrap_or(0)
}
fn water_need(content: &CnaContent, state: &State, id: &UnitId) -> Option<i32> {
    let r = water::requirements(content, state, id).ok()?;
    r.infantry.checked_add(r.activity)?.checked_add(r.pasta)
}
fn known_bad(state: &State, id: &UnitId, side: cna_protocol::Side) -> bool {
    let Some(hex) = state.land.units.get(id).and_then(|u| u.location.hex()) else {
        return false;
    };
    let condition = wells::condition(state, hex, Perspective::Side(side));
    condition["depleted"] == true || condition["poisoned"] == true
}
fn packing(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    stock: Supplies,
) -> Option<CargoPacking> {
    let unit = state.land.units.get(id)?;
    capacity::find_packing(content, &unit.trucks, &unit.transport_trucks, stock)
}
fn stored(state: &State, id: &UnitId) -> Supplies {
    state
        .logistics
        .unit_supply
        .get(id)
        .map_or(Supplies::default(), |h| h.carried)
}
fn well_reserve(content: &CnaContent, state: &State, id: &UnitId) -> i32 {
    let Some(unit) = state.land.units.get(id) else {
        return 0;
    };
    // First supply present friendly units, then enough for this unit's next stage.
    let other_need: i64 = state
        .land
        .units
        .values()
        .filter(|u| u.side == unit.side && u.location == unit.location && u.id != *id)
        .filter_map(|u| water_need(content, state, &u.id))
        .map(i64::from)
        .sum();
    let next = rations::activity_points(content, state, id)
        .ok()
        .and_then(|n| n.checked_add(i32::from(rations::infantry(content, id).unwrap_or(false))))
        .and_then(|n| n.checked_mul(rations::hot_multiplier(content, state, id).unwrap_or(1)))
        .unwrap_or(0);
    let desired = (other_need + i64::from(next)).min(i64::from(i32::MAX)) as i32;
    (desired - stored(state, id).water).max(0)
}
fn well_request(content: &CnaContent, state: &State, id: &UnitId) -> Option<(i32, CargoPacking)> {
    let need = water_need(content, state, id)?;
    let stock = stored(state, id);
    let mut low = 0i32;
    let mut high = well_reserve(content, state, id)
        .min(i32::MAX - need)
        .min(i32::MAX - stock.water);
    let mut best = packing(content, state, id, stock)?;
    while low < high {
        let mid = low + ((i64::from(high) - i64::from(low) + 1) / 2) as i32;
        let mut proposed = stock;
        proposed.water += mid;
        if let Some(p) = packing(content, state, id, proposed) {
            low = mid;
            best = p
        } else {
            high = mid - 1
        }
    }
    let requested = need.checked_add(low)?;
    if requested == 0 {
        return None;
    }
    Some((requested, best))
}

fn batch_units(request: &DecisionRequest, field_name: Option<&str>) -> Vec<UnitId> {
    let schema = if let Some(name) = field_name {
        let ActionSchema::Record { fields } = &request.space.schema else {
            return vec![];
        };
        let Some(f) = fields.iter().find(|f| f.name == name) else {
            return vec![];
        };
        &f.schema
    } else {
        &request.space.schema
    };
    let ActionSchema::List { item, .. } = schema else {
        return vec![];
    };
    let ActionSchema::Record { fields } = item.as_ref() else {
        return vec![];
    };
    let Some(f) = fields.iter().find(|f| f.name == "unit") else {
        return vec![];
    };
    let ActionSchema::Choice { options } = &f.schema else {
        return vec![];
    };
    options.iter().map(|o| UnitId::new(&o.id)).collect()
}
fn batch_orders(
    content: &CnaContent,
    state: &State,
    request: &DecisionRequest,
    rng: &mut CampaignRng,
) -> Option<Value> {
    use super::batches;
    let kind = request.kind.as_str();
    if kind == batches::DISTRIBUTION {
        return Some(Value::Null);
    }
    if ![batches::STORES, batches::WATER, batches::WELL_ALLOCATION].contains(&kind) {
        return None;
    }
    let mut draft = state.clone();
    let mut units = batch_units(
        request,
        if kind == batches::WATER {
            Some("allocations")
        } else {
            None
        },
    );
    let mut ranked: Vec<_> = units
        .drain(..)
        .map(|id| {
            let need = if kind == batches::STORES {
                rations::stores_required(content, &draft, &id).unwrap_or(0)
            } else {
                water_need(content, &draft, &id).unwrap_or(0)
            };
            (priority(content, &draft, &id), need, rng.d6().value(), id)
        })
        .collect();
    ranked.sort();
    let mut out = vec![];
    for (_, _, _, id) in ranked {
        if !draft
            .land
            .units
            .get(&id)
            .is_some_and(|u| u.side == request.seat.side)
        {
            continue;
        }
        let mut synthetic = request.clone();
        synthetic.kind = if kind == batches::STORES {
            format!("{}{id}", stores::ISSUE_PREFIX)
        } else if kind == batches::WATER {
            format!("{}{id}", water::ISSUE_PREFIX)
        } else {
            format!("{}{id}", wells::ALLOCATE_PREFIX)
        };
        synthetic.space.schema = ActionSchema::Record {
            fields: vec![stores::field(
                "stores",
                "Requirement",
                ActionSchema::Integer {
                    min: 0,
                    max: i64::from(rations::stores_required(content, &draft, &id).unwrap_or(0)),
                },
            )],
        };
        let Some(mut value) = logistics_orders(content, &draft, &synthetic, rng) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let read = |name: &str| {
            value[name]
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .unwrap_or(0)
        };
        let pasta = value["pasta"] == true;
        let applied = if kind == batches::STORES {
            if read("stores") == 0 {
                continue;
            }
            let ds: Vec<stores::RationDraw> =
                serde_json::from_value(value["draws"].clone()).ok()?;
            stores::issue_unit(
                content,
                &mut draft,
                &id,
                read("stores"),
                value["half"] == true,
                pasta,
                &stores::draws(ds).ok()?,
            )
        } else if kind == batches::WATER {
            if read("infantry") + read("activity") + i32::from(pasta) == 0 {
                continue;
            }
            let ds: Vec<stores::RationDraw> =
                serde_json::from_value(value["draws"].clone()).ok()?;
            water::issue_unit(
                content,
                &mut draft,
                &id,
                read("infantry"),
                read("activity"),
                pasta,
                &stores::draws(ds).ok()?,
            )
        } else {
            let allocation: wells::Allocation = serde_json::from_value(value.clone()).ok()?;
            wells::allocate(content, &mut draft, &id, &allocation)
        };
        if applied.is_err() {
            continue;
        }
        value.as_object_mut()?.insert("unit".into(), json!(id));
        out.push(value);
    }
    if kind != batches::WATER {
        return Some(json!(out));
    }
    let wells = batch_wells(content, &mut draft, request)?;
    if wells.is_null() {
        return Some(Value::Null);
    }
    Some(json!({"allocations":out,"wells":wells}))
}

fn batch_wells(
    content: &CnaContent,
    draft: &mut State,
    request: &DecisionRequest,
) -> Option<Value> {
    let mut ops = vec![];
    let ActionSchema::Record { fields } = &request.space.schema else {
        return Some(Value::Null);
    };
    let Some(f) = fields.iter().find(|f| f.name == "wells") else {
        return Some(Value::Null);
    };
    let ActionSchema::List { item, .. } = &f.schema else {
        return Some(Value::Null);
    };
    let ActionSchema::Record { fields } = item.as_ref() else {
        return Some(Value::Null);
    };
    let Some(f) = fields.iter().find(|f| f.name == "operation") else {
        return Some(Value::Null);
    };
    let ActionSchema::Choice { options } = &f.schema else {
        return Some(Value::Null);
    };
    for o in options {
        let Some(raw) = o.id.strip_prefix("draw|") else {
            continue;
        };
        let id = UnitId::new(raw);
        if !draft
            .land
            .units
            .get(&id)
            .is_some_and(|u| u.side == request.seat.side)
            || known_bad(draft, &id, request.seat.side)
            || draft.logistics.water_window.completed_wells.contains(&id)
        {
            continue;
        }
        let Some((requested, packing)) = well_request(content, draft, &id) else {
            continue;
        };
        if wells::prepare_draw(content, draft, &id, requested, &packing).is_err() {
            continue;
        }
        ops.push(json!({"operation":o.id,"requested":requested,"packing":packing}));
    }
    Some(json!(ops))
}

/// Feed and water the owning side from enumerated legal sources. Prefer units whose
/// restrictions can be relieved, then affordable smaller needs, with RNG tie-breaking.
/// Reserve water after immediate consumption; finite wells are never probed using enemy
/// hidden conditions. Other implemented logistics windows choose a declared pass, except
/// mandatory attrition, which chooses one of its legal casualty candidates.
/// The caller's RNG belongs to the controller; adjudication dice remain untouched.
/// Cases: airlog:51.11, airlog:51.23, airlog:52.13, airlog:52.41, airlog:52.42, airlog:52.6, land:3.6
pub fn logistics_orders_with_profile(
    content: &CnaContent,
    state: &State,
    request: &DecisionRequest,
    rng: &mut CampaignRng,
    strict: bool,
) -> Result<Option<Value>, cna_core::engine::EngineError> {
    if request.kind == super::truck_convoy::KIND {
        return Ok(Some(super::truck_convoy::baseline()));
    }
    let Some(mut answer) = logistics_orders(content, state, request, rng) else {
        return Ok(None);
    };
    if request.kind != super::batches::WATER || answer.is_null() {
        return Ok(Some(answer));
    }
    // Match the handler's unit -> pool -> well order on the disposable owner draft.
    // Pool stock issues can debit a well unit's carried water, changing its draw packing.
    answer["wells"] = json!([]);
    let mut draft = state.clone();
    super::batches::apply_water_answer(content, &mut draft, request.seat.side, &answer, strict)
        .map_err(|error| match error {
            cna_core::engine::Rejection::Engine(error) => error,
            _ => cna_core::engine::EngineError::Invariant {
                detail: "baseline water list is invalid".into(),
            },
        })?;
    let pools = water::pools::baseline(content, &mut draft, request.seat.side)?;
    answer["wells"] = batch_wells(content, &mut draft, request).ok_or_else(|| {
        cna_core::engine::EngineError::Invariant {
            detail: "baseline well list is invalid".into(),
        }
    })?;
    answer
        .as_object_mut()
        .ok_or_else(|| cna_core::engine::EngineError::Invariant {
            detail: "baseline water list is not an object".into(),
        })?
        .insert("pool_allocations".into(), json!(pools));
    Ok(Some(answer))
}

/// Legacy controller API; automatic pool reserve issue uses the fallible profile API.
pub fn logistics_orders(
    content: &CnaContent,
    state: &State,
    request: &DecisionRequest,
    rng: &mut CampaignRng,
) -> Option<Value> {
    if request.space.pass.is_some()
        && matches!(&request.space.schema, ActionSchema::Choice { options } if options.is_empty())
    {
        return Some(Value::Null);
    }
    if request.kind == super::arrivals::KIND {
        return Some(super::arrivals::baseline(
            content,
            state,
            request.seat.side,
            rng,
        ));
    }
    if let Some(answer) = batch_orders(content, state, request, rng) {
        return Some(answer);
    }
    let kind = request.kind.as_str();
    if kind == stores::KIND || kind == water::KIND {
        let ActionSchema::Choice { options } = &request.space.schema else {
            return Some(Value::Null);
        };
        let mut choices = Vec::new();
        for option in options {
            if option.id == "done" {
                continue;
            }
            let well = option.id.starts_with("well:");
            let id = UnitId::new(option.id.strip_prefix("well:").unwrap_or(&option.id));
            let Some(unit) = state
                .land
                .units
                .get(&id)
                .filter(|u| u.side == request.seat.side)
            else {
                continue;
            };
            let source = sources(content, state, &id);
            let need = if kind == stores::KIND {
                rations::stores_required(content, state, &id).ok()?
            } else {
                water_need(content, state, &id).unwrap_or(0)
            };
            if well {
                if known_bad(state, &id, request.seat.side)
                    || well_request(content, state, &id).is_none()
                {
                    continue;
                }
            } else if need == 0 || available(&source, kind == stores::KIND) == 0 {
                continue;
            }
            let restricted = priority(content, state, &id);
            // Wells cost CP; use already-available water before drawing another reserve.
            let rank = if well && need == 0 { 2 } else { restricted };
            choices.push((
                rank,
                if well { 1 } else { 0 },
                need,
                rng.d6().value(),
                unit.id.clone(),
                option.id.clone(),
            ));
        }
        choices.sort();
        return Some(choices.first().map_or(Value::Null, |c| json!(c.5)));
    }
    if let Some(raw) = kind.strip_prefix(stores::ISSUE_PREFIX) {
        let id = UnitId::new(raw);
        let source = sources(content, state, &id);
        let full = amount(request, "stores");
        let total = available(&source, true);
        let class = rations::class(content, &id).ok()?;
        let half = total < i64::from(full)
            && total >= i64::from(full / 2)
            && full > 0
            && !matches!(class.unit_type.as_str(), "headquarters" | "engineer");
        let food = if half {
            full / 2
        } else {
            full.min(total.min(i64::from(i32::MAX)) as i32)
        };
        let pasta = food > 0 && rations::pasta(content, &id) && available(&source, false) > 0;
        return Some(
            json!({"stores":food,"half":half,"pasta":pasta,"draws":draws(&source,food,i32::from(pasta))}),
        );
    }
    if let Some(raw) = kind.strip_prefix(water::ISSUE_PREFIX) {
        let id = UnitId::new(raw);
        let source = sources(content, state, &id);
        let r = water::requirements(content, state, &id).ok()?;
        let mut remaining = available(&source, false).min(i64::from(i32::MAX)) as i32;
        let infantry = r.infantry.min(remaining);
        remaining -= infantry;
        let pasta = r.pasta == 1 && remaining > 0;
        remaining -= i32::from(pasta);
        let activity = r.activity.min(remaining);
        return Some(
            json!({"infantry":infantry,"activity":activity,"pasta":pasta,
            "draws":draws(&source,0,infantry+activity+i32::from(pasta))}),
        );
    }
    if let Some(raw) = kind.strip_prefix(wells::REQUEST_PREFIX) {
        let id = UnitId::new(raw);
        return Some(
            well_request(content, state, &id)
                .map_or(Value::Null, |(n, p)| json!({"requested":n,"packing":p})),
        );
    }
    if let Some(raw) = kind.strip_prefix(wells::ALLOCATE_PREFIX) {
        let id = UnitId::new(raw);
        let r = water::requirements(content, state, &id).ok()?;
        let mut available = state.logistics.drawn_water.get(&id)?.points;
        let infantry = r.infantry.min(available);
        available -= infantry;
        let pasta = r.pasta == 1 && available > 0;
        available -= i32::from(pasta);
        let activity = r.activity.min(available);
        available -= activity;
        let mut stock = stored(state, &id);
        stock.water = stock.water.checked_add(available)?;
        let p = packing(content, state, &id, stock)?;
        return Some(
            json!({"infantry":infantry,"activity":activity,"pasta":pasta,"cargo":available,"packing":p}),
        );
    }
    if kind.starts_with(wells::PREFIX) {
        let ActionSchema::Choice { options } = &request.space.schema else {
            return Some(Value::Null);
        };
        return Some(if options.iter().any(|o| o.id == "draw") {
            json!("draw")
        } else {
            Value::Null
        });
    }
    if kind == attrition::KIND {
        return attrition::baseline(content, state, request.seat.side);
    }
    if kind == distribution::KIND
        || kind.starts_with(distribution::PREFIX)
        || kind == super::coastal::AXIS
        || kind == super::coastal::CW
        || kind.starts_with(super::convoys::PREFIX)
    {
        return request.space.pass.as_ref().map(|_| Value::Null);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Cna,
        seq::Block,
        state::{Dump, DumpLocation, Location, WeatherState},
    };
    use cna_core::{
        decision::DecisionResponse,
        engine::{Command, Cx, Game, Ruleset, evaluate},
    };
    use cna_tables::land::weather::WeatherKind;
    use std::sync::OnceLock;
    fn content() -> &'static CnaContent {
        static CONTENT: OnceLock<CnaContent> = OnceLock::new();
        CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
    }
    fn fixture(seed: u8) -> State {
        let mut s = State::new(content()).unwrap();
        let ids: Vec<UnitId> = [
            "cw.2_nz_div.21st_nz_bn",
            "cw.unassigned_inf.1st_rnf_mg_bn",
            "it.1_libyan_div.viii_libyan_bn",
            "it.libyan_tank_command.xxi_l_tank_bn",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        for (id, u) in &mut s.land.units {
            u.location = if ids.contains(id) {
                Location::Hex {
                    hex: if u.side == cna_protocol::Side::Axis {
                        "C4020".into()
                    } else {
                        "E1730".into()
                    },
                }
            } else {
                Location::Eliminated
            };
            u.trucks = cna_content::units::Trucks::default();
            u.transport_trucks = cna_content::units::Trucks::default();
        }
        s.turn.weather = Some(WeatherState {
            kind: WeatherKind::Normal,
            storm_sections: vec![],
        });
        s.cursor.block = Block::Pre;
        s.cursor.index = 9;
        s.cursor.op_stage = Some(1);
        s.logistics.dumps.insert(
            "fixture-stock".into(),
            Dump {
                marker: String::new(),
                id: "fixture-stock".into(),
                side: cna_protocol::Side::Axis,
                location: DumpLocation::Hex {
                    hex: "C4020".into(),
                },
                supplies: Supplies {
                    stores: i32::from(seed) % 31 + 1,
                    water: 40,
                    ..Supplies::default()
                },
                active: true,
                dummy: false,
            },
        );
        s
    }
    fn drain(
        s: &mut State,
        controller: &mut CampaignRng,
        dice: &mut CampaignRng,
        seen: &mut std::collections::BTreeSet<String>,
    ) {
        for _ in 0..256 {
            if s.decisions.pending.is_empty() {
                if s.logistics
                    .allocation_batches
                    .submitted
                    .keys()
                    .any(|k| k.starts_with(crate::logistics::batches::STORES))
                {
                    crate::logistics::batches::finish_stores(
                        content(),
                        s,
                        &mut Cx {
                            rng: dice,
                            events: &mut vec![],
                        },
                    )
                    .unwrap();
                }
                if s.logistics
                    .allocation_batches
                    .submitted
                    .keys()
                    .any(|k| k.starts_with(crate::logistics::batches::DISTRIBUTION))
                {
                    crate::logistics::batches::finish_distribution(
                        content(),
                        s,
                        &mut Cx {
                            rng: dice,
                            events: &mut vec![],
                        },
                    )
                    .unwrap();
                }
                crate::logistics::batches::finish_water(
                    content(),
                    s,
                    &mut Cx {
                        rng: dice,
                        events: &mut vec![],
                    },
                    false,
                )
                .unwrap();
            }
            let Some(p) = s.decisions.pending.first().cloned() else {
                return;
            };
            let request = Cna::dev().pending(content(), s).remove(0);
            let before = serde_json::to_value(&*s).unwrap();
            let dice_before = dice.state();
            let action =
                logistics_orders(content(), s, &request, controller).expect("owned logistics kind");
            assert_eq!(serde_json::to_value(&*s).unwrap(), before);
            assert_eq!(dice.state(), dice_before);
            seen.insert(p.kind.clone());
            s.decisions.pending.remove(0);
            let mut events = Vec::new();
            let mut cx = Cx {
                rng: dice,
                events: &mut events,
            };
            let result = Cna::dev().respond_to(content(), s, &p, &action, &mut cx);
            result.unwrap_or_else(|e| panic!("{} generated {action}: {e:?}", p.kind));
        }
        panic!("baseline did not finish the logistics window")
    }
    /// Cases: airlog:52.13, airlog:52.42, land:3.6
    #[test]
    fn pool_stock_issue_precedes_well_packing_in_the_joint_baseline() {
        let strict = false;
        let id = UnitId::new("it.libyan_tank_command.xxi_l_tank_bn");
        let mut state = fixture(0);
        for (other, unit) in &mut state.land.units {
            if *other != id {
                unit.location = Location::Eliminated;
            }
        }
        state.logistics.dumps.clear();
        state.logistics.unit_supply.clear();
        state.cursor.block = Block::OpStage;
        state.cursor.index = 2;
        state.land.units.get_mut(&id).unwrap().location = Location::Hex {
            hex: "E1730".into(),
        };
        state.land.units.get_mut(&id).unwrap().trucks.light = 1;
        assert_eq!(
            wells::source_at(
                content(),
                &state,
                cna_protocol::Side::Axis,
                &state.land.units[&id].location
            )
            .unwrap(),
            wells::Source::MajorCity
        );
        let need = rations::activity_points(content(), &state, &id).unwrap();
        assert!(need > 1);
        let stock = Supplies {
            water: 1,
            ..Supplies::default()
        };
        assert!(packing(content(), &state, &id, stock).is_some());
        let held = state.logistics.unit_supply.entry(id.clone()).or_default();
        held.carried = stock;
        held.activity_water = cna_core::quantity::WaterPoints::new(need);
        let pool = crate::logistics::pools::add_truck_pool(
            &mut state.logistics,
            None,
            cna_protocol::Side::Axis,
            cna_content::scenario::Placement::Hex {
                hex: "E1730".into(),
            },
            Some(state.land.units[&id].location.clone()),
            cna_content::units::Trucks {
                light: 1,
                ..Default::default()
            },
            Supplies::default(),
        )
        .unwrap();
        let mut rng = CampaignRng::from_seed([13; 32]);
        let mut events = vec![];
        let rules = Cna::dev();
        water::enter(
            content(),
            &mut state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
            strict,
        )
        .unwrap();
        let before = serde_json::to_value(&state).unwrap();
        let game_rng = rng.state();
        let request = rules.pending(content(), &state).remove(0);
        assert_eq!(request.kind, crate::logistics::batches::WATER);
        let mut controller = CampaignRng::from_seed([17; 32]);
        let legacy =
            logistics_orders(content(), &state, &request, &mut controller.clone()).unwrap();
        let action =
            logistics_orders_with_profile(content(), &state, &request, &mut controller, strict)
                .unwrap()
                .unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        assert_eq!(rng.state(), game_rng);
        assert_eq!(action["pool_allocations"][0]["pool"], json!(pool));
        assert_eq!(action["pool_allocations"][0]["activity"], 1);
        assert_eq!(
            action["pool_allocations"][0]["draws"][0]["source"],
            json!(serde_json::to_string(&SupplySource::UnitStock(id.clone())).unwrap())
        );
        assert_eq!(action["wells"].as_array().unwrap().len(), 1);
        assert_eq!(
            action["wells"][0]["requested"].as_i64().unwrap(),
            legacy["wells"][0]["requested"].as_i64().unwrap() + 1
        );
        let respond = |game: &Game<Cna>, action: Value| {
            let request = rules.pending(content(), &game.state).remove(0);
            let before = serde_json::to_value(game).unwrap();
            let transition = evaluate(
                &rules,
                content(),
                game,
                &Command::Respond(DecisionResponse {
                    decision_id: request.id.clone(),
                    seat: request.seat,
                    controller_epoch: 1,
                    decision_revision: request.revision,
                    idempotency_key: request.id.to_string(),
                    action,
                    public_explanation: None,
                }),
            )
            .unwrap();
            assert_eq!(serde_json::to_value(game).unwrap(), before);
            assert_eq!(transition.game.rng, game.rng);
            transition.game
        };
        let mut game = respond(
            &Game {
                state,
                rng: game_rng.clone(),
            },
            action.clone(),
        );
        assert_eq!(game.state.logistics.unit_supply[&id].carried.water, 1);
        assert_eq!(
            game.state
                .logistics
                .truck_pools
                .iter()
                .find(|p| p.id == pool)
                .unwrap()
                .activity_water
                .get(),
            0
        );
        assert_eq!(game.state.land.units[&id].cp_spent_quarters, 0);
        assert!(game.state.logistics.drawn_water.is_empty());
        assert_eq!(game.state.decisions.pending.len(), 1);
        let other = rules.pending(content(), &game.state).remove(0);
        let pass =
            logistics_orders_with_profile(content(), &game.state, &other, &mut controller, strict)
                .unwrap()
                .unwrap();
        game = respond(&game, pass);
        assert!(game.state.decisions.pending.is_empty());

        // Both fixed rounds replay from full game checkpoints, including events and dice.
        let finish = |game: &Game<Cna>| {
            let mut state = game.state.clone();
            let mut dice = CampaignRng::from_state(&game.rng);
            let mut events = vec![];
            crate::logistics::batches::finish_water(
                content(),
                &mut state,
                &mut Cx {
                    rng: &mut dice,
                    events: &mut events,
                },
                strict,
            )
            .unwrap();
            (
                Game::<Cna> {
                    state,
                    rng: dice.state(),
                },
                events,
            )
        };
        let checkpoint: Game<Cna> =
            serde_json::from_slice(&serde_json::to_vec(&game).unwrap()).unwrap();
        let (closed, events) = finish(&game);
        let (replayed, replay_events) = finish(&checkpoint);
        assert_eq!(
            serde_json::to_value(&closed).unwrap(),
            serde_json::to_value(&replayed).unwrap()
        );
        assert_eq!(
            serde_json::to_value(events).unwrap(),
            serde_json::to_value(replay_events).unwrap()
        );
        game = closed;
        let drawn = game.state.logistics.drawn_water[&id].points;
        assert_eq!(
            drawn,
            action["wells"][0]["requested"].as_i64().unwrap() as i32
        );
        assert_eq!(game.state.logistics.unit_supply[&id].carried.water, 0);
        assert_eq!(
            game.state.logistics.unit_supply[&id].activity_water.get(),
            0
        );
        assert_eq!(
            game.state
                .logistics
                .truck_pools
                .iter()
                .find(|p| p.id == pool)
                .unwrap()
                .activity_water
                .get(),
            1
        );
        assert_eq!(game.state.land.units[&id].cp_spent_quarters, 4);
        assert!(game.state.logistics.dumps.is_empty());
        assert_eq!(game.state.decisions.pending.len(), 2);
        for _ in 0..2 {
            let request = rules.pending(content(), &game.state).remove(0);
            assert_eq!(request.kind, crate::logistics::batches::WELL_ALLOCATION);
            let action = logistics_orders_with_profile(
                content(),
                &game.state,
                &request,
                &mut controller,
                strict,
            )
            .unwrap()
            .unwrap();
            game = respond(&game, action);
        }
        let checkpoint: Game<Cna> =
            serde_json::from_slice(&serde_json::to_vec(&game).unwrap()).unwrap();
        let (closed, events) = finish(&game);
        let (replayed, replay_events) = finish(&checkpoint);
        assert_eq!(
            serde_json::to_value(&closed).unwrap(),
            serde_json::to_value(&replayed).unwrap()
        );
        assert_eq!(
            serde_json::to_value(events).unwrap(),
            serde_json::to_value(replay_events).unwrap()
        );
        let holding = &closed.state.logistics.unit_supply[&id];
        let reserve = closed
            .state
            .logistics
            .truck_pools
            .iter()
            .find(|p| p.id == pool)
            .unwrap()
            .activity_water
            .get();
        let paid = closed.state.logistics.rations[&id]
            .activity_water_ledger
            .as_ref()
            .unwrap();
        assert_eq!(paid.body_paid + paid.truck_paid.light, need);
        assert_eq!(
            1 + need + drawn,
            holding.carried.water + holding.activity_water.get() + reserve + need
        );
        assert_eq!(holding.carried.water, drawn);
        assert_eq!(holding.activity_water.get(), 0);
        assert_eq!(reserve, 1);
        assert!(closed.state.logistics.drawn_water.is_empty());
        assert!(closed.state.decisions.pending.is_empty());
        assert!(matches!(
            closed.state.logistics.water_window.round,
            crate::logistics::batches::WaterRound::Complete
        ));
    }

    /// Cases: airlog:51.11, airlog:51.23, airlog:52.13, airlog:52.41, airlog:52.42, airlog:52.6
    #[test]
    fn generated_rations_and_water_are_accepted_across_many_seeds() {
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..64u8 {
            let mut s = fixture(seed);
            let mut controller = CampaignRng::from_seed([seed; 32]);
            let mut dice = CampaignRng::from_seed([seed.wrapping_add(127); 32]);
            let mut events = Vec::new();
            stores::enter(
                content(),
                &mut s,
                &mut Cx {
                    rng: &mut dice,
                    events: &mut events,
                },
            )
            .unwrap();
            drain(&mut s, &mut controller, &mut dice, &mut seen);
            for id in ["cw.2_nz_div.21st_nz_bn", "cw.unassigned_inf.1st_rnf_mg_bn"] {
                let h = &s.logistics.rations[&UnitId::new(id)];
                assert_eq!(h.stores_received, h.stores_required);
                assert!(!h.half);
            }
            s.cursor.block = Block::OpStage;
            s.cursor.index = 2;
            water::enter(
                content(),
                &mut s,
                &mut Cx {
                    rng: &mut dice,
                    events: &mut events,
                },
                false,
            )
            .unwrap();
            drain(&mut s, &mut controller, &mut dice, &mut seen);
            for id in ["cw.2_nz_div.21st_nz_bn", "cw.unassigned_inf.1st_rnf_mg_bn"] {
                assert_eq!(
                    s.logistics.rations[&UnitId::new(id)].infantry_water_received,
                    1
                );
                assert_eq!(water_need(content(), &s, &UnitId::new(id)), Some(0));
            }
            let tank = UnitId::new("it.libyan_tank_command.xxi_l_tank_bn");
            assert!(s.logistics.unit_supply[&tank].activity_water.get() > 0);
            assert!(
                movement_restrictions(content(), &s, &tank)
                    .unwrap()
                    .may_move
            );
        }
        assert!(seen.contains(crate::logistics::batches::STORES));
        assert!(seen.contains(crate::logistics::batches::WATER));

        assert!(seen.contains(crate::logistics::batches::WELL_ALLOCATION));
    }
    /// Cases: land:3.6, airlog:52.14, airlog:52.16
    #[test]
    fn policy_is_reproducible_and_cannot_see_secret_enemy_well_conditions() {
        let mut s = fixture(22);
        s.cursor.block = Block::OpStage;
        s.cursor.index = 2;
        let mut dice = CampaignRng::from_seed([99; 32]);
        let mut events = Vec::new();
        water::enter(
            content(),
            &mut s,
            &mut Cx {
                rng: &mut dice,
                events: &mut events,
            },
            false,
        )
        .unwrap();
        let request = Cna::dev()
            .pending(content(), &s)
            .into_iter()
            .find(|r| r.seat.side == cna_protocol::Side::Commonwealth)
            .unwrap();
        let before = serde_json::to_value(&s).unwrap();
        let mut a = CampaignRng::from_seed([1; 32]);
        let mut b = CampaignRng::from_seed([1; 32]);
        let action = logistics_orders(content(), &s, &request, &mut a);
        assert_eq!(action, logistics_orders(content(), &s, &request, &mut b));
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        let w = s.logistics.wells.entry("E1730".into()).or_default();
        w.poisoned = true;
        w.depleted = true;
        w.poisoned_known.insert(cna_protocol::Side::Axis);
        w.depleted_known.insert(cna_protocol::Side::Axis);
        let mut c = CampaignRng::from_seed([1; 32]);
        assert_eq!(action, logistics_orders(content(), &s, &request, &mut c));
    }
    fn first(schema: &ActionSchema) -> Value {
        match schema {
            ActionSchema::Choice { options } => json!(options[0].id),
            ActionSchema::Unit { among } => json!(among[0]),
            ActionSchema::Integer { max, .. } => json!(max),
            ActionSchema::Bool => json!(false),
            ActionSchema::Record { fields } => Value::Object(
                fields
                    .iter()
                    .map(|f| (f.name.clone(), first(&f.schema)))
                    .collect(),
            ),
            ActionSchema::List { .. } => json!([]),
            _ => Value::Null,
        }
    }
    /// Actual setup, pregame plans, stores, weather and organization precede the move.
    /// Cases: airlog:48.0, airlog:51.11, airlog:52.41, airlog:52.42, land:8.11
    #[test]
    fn real_graziani_campaign_feeds_waters_and_moves_after_first_organization() {
        let c = content();
        let rules = Cna::dev();
        let mut g: Game<Cna> = Game {
            state: State::new(c).unwrap(),
            rng: CampaignRng::from_seed([41; 32]).state(),
        };
        let mut controller = CampaignRng::from_seed([11; 32]);
        let mut fed = 0;
        let mut watered = 0;
        for serial in 0..2500 {
            let transition = evaluate(&rules, c, &g, &Command::Advance).unwrap();
            g = transition.game;
            let request = rules.pending(c, &g.state).remove(0);
            let action = if let Some(a) = logistics_orders(c, &g.state, &request, &mut controller) {
                a
            } else if request.kind == crate::land::movement::KIND {
                crate::baseline::random_orders(c, &g.state, &request, &mut controller)
            } else if request.space.pass.is_some() {
                Value::Null
            } else {
                first(&request.space.schema)
            };
            let is_move = request.kind == crate::land::movement::KIND;
            let response = DecisionResponse {
                decision_id: request.id.clone(),
                seat: request.seat,
                controller_epoch: 1,
                decision_revision: request.revision,
                idempotency_key: format!("baseline-{serial}"),
                action: action.clone(),
                public_explanation: None,
            };
            let transition = evaluate(&rules, c, &g, &Command::Respond(response))
                .unwrap_or_else(|e| panic!("{} generated {action}: {e:?}", request.kind));
            g = transition.game;
            fed = fed.max(
                g.state
                    .logistics
                    .rations
                    .values()
                    .filter(|r| {
                        r.stores_received > 0 && !r.half && r.stores_received == r.stores_required
                    })
                    .count(),
            );
            watered = watered.max(
                g.state
                    .logistics
                    .rations
                    .values()
                    .filter(|r| r.infantry_water_received > 0)
                    .count(),
            );
            if is_move
                && transition
                    .events
                    .iter()
                    .any(|e| matches!(e.event, cna_protocol::GameEvent::UnitMoved { .. }))
            {
                assert_eq!(g.state.cursor.game_turn, 1);
                assert_eq!(g.state.cursor.op_stage, Some(1));
                assert!(fed > 0);
                assert!(watered > 0);
                return;
            }
        }
        panic!("no real move after the first organization phase")
    }
}
