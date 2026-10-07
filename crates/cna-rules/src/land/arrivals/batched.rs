//! Fixed role windows hide whether either side has arrivals or withdrawal choices.
use super::*;
use cna_core::decision::DecisionRequest;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Window {
    stage: String,
    jobs: BTreeMap<SeatId, Vec<Job>>,
    answers: BTreeMap<SeatId, Value>,
    resolved: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Job {
    Place {
        row: String,
        unit: UnitId,
        destinations: Vec<Location>,
    },
    Pool {
        row: String,
        destinations: Vec<Location>,
    },
    Trucks {
        row: String,
        units: Vec<UnitId>,
        available: Trucks,
    },
    Substitute {
        row: String,
        named: UnitId,
        candidates: Vec<UnitId>,
    },
    Transport {
        row: String,
        assets: BTreeMap<String, Trucks>,
    },
    Air {
        row: String,
        quotas: Vec<i32>,
        remaining: BTreeMap<String, i32>,
    },
}
impl Window {
    #[cfg(test)]
    pub(super) fn is_resolved(&self) -> bool {
        self.resolved
    }
}
impl Job {
    fn key(&self) -> String {
        match self {
            Self::Place { unit, .. } => format!("place:{unit}"),
            Self::Pool { row, .. } => format!("pool:{row}"),
            Self::Trucks { row, .. } => format!("trucks:{row}"),
            Self::Substitute { row, named, .. } => format!("substitute:{row}:{named}"),
            Self::Transport { row, .. } => format!("transport:{row}"),
            Self::Air { row, .. } => format!("air:{row}"),
        }
    }
    fn schema(&self) -> ActionSchema {
        match self {
            Self::Place { destinations, .. } | Self::Pool { destinations, .. } => {
                let mut choices: Vec<_> = destinations
                    .iter()
                    .map(|l| {
                        let id = crate::setup::placement::destination_id(l).unwrap();
                        (id.clone(), id)
                    })
                    .collect();
                if matches!(self, Self::Place { .. }) {
                    choices.push(("await_capacity".into(), "Await capacity only if no destination remains legal after this owner's earlier placements".into()));
                }
                options(choices).schema
            }
            Self::Trucks {
                units, available, ..
            } => record(
                units
                    .iter()
                    .map(|u| {
                        field(
                            u.to_string(),
                            "Truck allocation to this arriving unit",
                            record(truck_fields(*available)),
                        )
                    })
                    .collect(),
            ),
            Self::Substitute { candidates, .. } => {
                options(
                    candidates
                        .iter()
                        .map(|u| (u.to_string(), format!("Substitute {u}")))
                        .chain(std::iter::once((
                            "eliminate".into(),
                            "Accept the printed deadline elimination".into(),
                        )))
                        .collect(),
                )
                .schema
            }
            Self::Transport { assets, .. } => record(vec![field(
                "priority",
                "Rank empty holding/type pairs; remaining holdings follow in stable order. Closure withdraws the required minimum after accompanying trucks.",
                ActionSchema::List {
                    item: Box::new(
                        options(
                            transport_options(assets)
                                .into_iter()
                                .map(|s| (s.clone(), s))
                                .collect(),
                        )
                        .schema,
                    ),
                    min: 0,
                    max: (assets.len() * 3) as u32,
                },
            )]),
            Self::Air {
                quotas, remaining, ..
            } => record(vec![
                field(
                    "quota",
                    "Balanced quota for this week",
                    options(
                        quotas
                            .iter()
                            .map(|n| (n.to_string(), format!("{n} aircraft this week")))
                            .collect(),
                    )
                    .schema,
                ),
                field(
                    "planes",
                    "Aircraft arriving this Operations Stage; unallocated quota is deferred",
                    record(
                        remaining
                            .iter()
                            .map(|(p, n)| {
                                field(
                                    p.clone(),
                                    "Aircraft count",
                                    ActionSchema::Integer {
                                        min: 0,
                                        max: i64::from(*n),
                                    },
                                )
                            })
                            .collect(),
                    ),
                ),
            ]),
        }
    }
}
fn field(name: impl Into<String>, doc: &str, schema: ActionSchema) -> Field {
    Field {
        name: name.into(),
        doc: doc.into(),
        schema,
        optional: false,
    }
}
fn record(fields: Vec<Field>) -> ActionSchema {
    ActionSchema::Record { fields }
}
fn seats() -> impl Iterator<Item = SeatId> {
    Side::ALL
        .into_iter()
        .flat_map(|s| [Role::Commander, Role::Logistics, Role::Air].map(move |r| SeatId::new(s, r)))
}
fn insert(jobs: &mut BTreeMap<SeatId, Vec<Job>>, side: Side, role: Role, job: Job) {
    jobs.get_mut(&SeatId::new(side, role)).unwrap().push(job);
}
fn due_withdrawals(
    content: &CnaContent,
    state: &State,
) -> Vec<(String, Side, BTreeSet<UnitId>, bool)> {
    content
        .units
        .schedules
        .iter()
        .flat_map(|s| {
            s.withdrawals.iter().enumerate().filter_map(move |(i, r)| {
                if r.gt != Some(state.cursor.game_turn)
                    || r.opstage != state.cursor.op_stage
                    || r.units.is_empty()
                {
                    return None;
                }
                let id = row_id(&s.path, "withdrawal", i);
                if state.land.arrivals.applied.contains(&id) {
                    return None;
                }
                Some((
                    id,
                    s.file.side,
                    expand(content, &r.units, None),
                    r.transport.is_some_and(|t| t.truck_value_points > 0),
                ))
            })
        })
        .collect()
}
/// Always the same six role windows. Internal air menus never stamp public decision counters.
/// Cases: land:3.6, land:20.12, land:20.14, land:20.83, land:20.85, airlog:34.84
pub(super) fn enter(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let mut jobs: BTreeMap<_, _> = seats().map(|seat| (seat, Vec::new())).collect();
    // Initialize the existing source-balanced air ledgers without exposing intermediate requests.
    let decisions = state.decisions.clone();
    let tasks = state.land.arrivals.tasks.clone();
    let mut private_events = Vec::new();
    air_start(
        content,
        state,
        &mut Cx {
            rng: cx.rng,
            events: &mut private_events,
        },
    )?;
    state.decisions = decisions;
    state.land.arrivals.tasks = tasks;
    for (row, b) in &state.land.arrivals.batches {
        if state.land.arrivals.applied.contains(row) {
            continue;
        }
        let domain = city_domain(content, &b.city)?;
        for unit in &b.units {
            if !waiting_at_arrival(&state.land.units[unit].location) {
                continue;
            }
            let mut destinations = Vec::new();
            for l in &domain {
                match valid_destination(content, state, unit, l, false) {
                    Ok(_) => destinations.push(l.clone()),
                    Err(Rejection::Illegal { .. }) => {}
                    Err(Rejection::Engine(e)) => return Err(e),
                    Err(_) => return Err(invariant("unexpected arrival destination validation")),
                }
            }
            if destinations.is_empty() {
                if strict {
                    return Err(EngineError::Unsupported {
                        case: "land:20.14".into(),
                        detail: "no verified capacity-valid arrival destination".into(),
                    });
                }
                unit_note(
                    cx,
                    b.side,
                    unit,
                    &state.land.units[unit].location,
                    format!("{unit} awaits a verified arrival destination (land:20.14)."),
                );
            } else if !(destinations.len() == 1
                && matches!(destinations[0], Location::OffMap { .. }))
            {
                insert(
                    &mut jobs,
                    b.side,
                    Role::Commander,
                    Job::Place {
                        row: row.clone(),
                        unit: unit.clone(),
                        destinations,
                    },
                );
            }
        }
        if b.trucks.total() > 0 {
            if b.alone || b.units.is_empty() {
                if domain.len() > 1 {
                    insert(
                        &mut jobs,
                        b.side,
                        Role::Logistics,
                        Job::Pool {
                            row: row.clone(),
                            destinations: domain,
                        },
                    );
                }
            } else {
                insert(
                    &mut jobs,
                    b.side,
                    Role::Logistics,
                    Job::Trucks {
                        row: row.clone(),
                        units: b.units.clone(),
                        available: b.trucks,
                    },
                );
            }
        }
    }
    let reserved_named: BTreeSet<_> = due_withdrawals(content, state)
        .into_iter()
        .flat_map(|(_, _, units, _)| units)
        .collect();
    for (row, side, units, transport) in due_withdrawals(content, state) {
        state
            .land
            .arrivals
            .withdrawals
            .entry(row.clone())
            .or_default();
        for named in units {
            if matches!(content.units.units[&named].arrives,Arrival::At {gt,opstage} if (gt,opstage)>(state.cursor.game_turn,state.cursor.op_stage.unwrap()))
            {
                continue;
            }
            if !ready_for_withdrawal(content, state, &named) {
                let candidates = substitutes(content, state, &named, &row)
                    .into_iter()
                    .filter(|id| !reserved_named.contains(id))
                    .collect::<Vec<_>>();
                if !candidates.is_empty() {
                    insert(
                        &mut jobs,
                        side,
                        Role::Commander,
                        Job::Substitute {
                            row: row.clone(),
                            named,
                            candidates,
                        },
                    );
                }
            }
        }
        if transport && !empty_trucks(content, state).is_empty() {
            insert(
                &mut jobs,
                side,
                Role::Logistics,
                Job::Transport {
                    row,
                    assets: empty_trucks(content, state),
                },
            );
        }
    }
    for (row, remaining) in &state.land.arrivals.air_remaining {
        if state.land.arrivals.applied.contains(row)
            || state.land.arrivals.air_deferred.get(row)
                == Some(&(state.cursor.game_turn, state.cursor.op_stage.unwrap()))
        {
            continue;
        }
        let Some((side, r)) = air_row(content, row) else {
            continue;
        };
        let from = r.gt_from.or(r.gt).unwrap();
        let to = r.gt_to.or(r.gt).unwrap();
        let gt = state.cursor.game_turn;
        if !(from..=to).contains(&gt) || r.opstage.is_some_and(|o| Some(o) != state.cursor.op_stage)
        {
            continue;
        }
        let quotas = if let Some((g, n)) = state
            .land
            .arrivals
            .air_week
            .get(row)
            .filter(|(g, _)| *g == gt)
        {
            let _ = g;
            vec![*n]
        } else {
            let weeks = i32::from(to - gt + 1);
            let base = r.planes.iter().map(|p| p.n).sum::<i32>() / i32::from(to - from + 1);
            let extra = remaining.values().sum::<i32>() - base * weeks;
            if extra == 0 {
                vec![base]
            } else if extra == weeks {
                vec![base + 1]
            } else {
                vec![base + 1, base]
            }
        };
        if quotas == [0] {
            continue;
        }
        insert(
            &mut jobs,
            side,
            Role::Air,
            Job::Air {
                row: row.clone(),
                quotas,
                remaining: remaining.clone(),
            },
        );
    }
    state.land.arrivals.window = Some(Window {
        stage: stage(state),
        jobs: jobs.clone(),
        answers: BTreeMap::new(),
        resolved: false,
    });
    for seat in seats() {
        let own = &jobs[&seat];
        let mut space = ActionSpace::new(record(
            own.iter()
                .map(|j| field(j.key(), "One complete owner order", j.schema()))
                .collect(),
        ));
        if own.is_empty() {
            space.schema = ActionSchema::Choice { options: vec![] };
            space =
                space.with_pass("No orders for this role; acknowledge the shared arrival barrier.");
        }
        space =
            space.with_context(json!({"forced_pass":own.is_empty(),"arrival_stage":stage(state)}));
        open(
            state,
            cx,
            seat,
            BATCH,
            "Submit this role's complete arrival and withdrawal orders.".into(),
            &["land:20.12", "land:20.83", "airlog:34.84"],
            Trigger::Scheduled,
            Secrecy::SecretSimultaneous,
            space,
        );
    }
    Ok(())
}
fn transport_options(assets: &BTreeMap<String, Trucks>) -> Vec<String> {
    assets
        .iter()
        .flat_map(|(id, t)| {
            [("light", t.light), ("medium", t.medium), ("heavy", t.heavy)]
                .into_iter()
                .filter(|(_, n)| *n > 0)
                .map(move |(kind, _)| format!("{id}|{kind}"))
        })
        .collect()
}
fn counts(value: &Value) -> Result<Trucks, Rejection> {
    let n = |k| {
        value
            .get(k)
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
            .filter(|n| *n >= 0)
            .ok_or_else(|| illegal("invalid truck points"))
    };
    Ok(Trucks {
        light: n("light")?,
        medium: n("medium")?,
        heavy: n("heavy")?,
    })
}
fn bounded(t: Trucks, max: Trucks) -> bool {
    t.light <= max.light && t.medium <= max.medium && t.heavy <= max.heavy
}
fn chosen<'a>(value: &Value, domain: &'a [Location]) -> Result<&'a Location, Rejection> {
    domain
        .iter()
        .find(|l| crate::setup::placement::destination_id(l).as_deref() == value.as_str())
        .ok_or_else(|| illegal("choose an advertised arrival destination"))
}
/// Validation reads only this role's advertised domains, own holdings and public geography.
/// Cases: land:3.6, land:20.12, land:20.14, land:4.43, airlog:34.84
pub(super) fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    strict: bool,
) -> Result<String, Rejection> {
    pending.space.check(action).map_err(illegal)?;
    let w = state
        .land
        .arrivals
        .window
        .as_ref()
        .ok_or_else(|| illegal("arrival window is not current"))?;
    if w.stage != stage(state) || w.resolved || w.answers.contains_key(&pending.seat) {
        return Err(illegal("arrival window is not current"));
    }
    let jobs = w
        .jobs
        .get(&pending.seat)
        .ok_or_else(|| illegal("not an arrival role"))?;
    let mut own = state.clone();
    let mut selected = BTreeSet::new();
    for j in jobs {
        let a = &action[&j.key()];
        match j {
            Job::Place {
                unit, destinations, ..
            } => {
                if a.as_str() == Some("await_capacity") {
                    for destination in destinations {
                        match valid_destination(content, &own, unit, destination, strict) {
                            Ok(_) => {
                                return Err(illegal(
                                    "arrival delay requires every destination to lack capacity",
                                ));
                            }
                            Err(Rejection::Illegal { .. }) => {}
                            Err(e) => return Err(e),
                        }
                    }
                    continue;
                }
                let l = chosen(a, destinations)?;
                valid_destination(content, &own, unit, l, strict)?;
                own.land.units.get_mut(unit).unwrap().location = l.clone();
            }
            Job::Pool { destinations, .. } => {
                chosen(a, destinations)?;
            }
            Job::Trucks {
                units, available, ..
            } => {
                let mut sum = Trucks::default();
                for u in units {
                    let t = counts(&a[u.as_str()])?;
                    if !bounded(t, *available) {
                        return Err(illegal("truck allocation exceeds its printed row"));
                    }
                    add(&mut sum, t)?;
                }
                if sum != *available {
                    return Err(illegal("distribute every printed truck point exactly once"));
                }
            }
            Job::Substitute {
                candidates,
                named,
                row,
            } => {
                let s = a.as_str().ok_or_else(|| illegal("choose a substitute"))?;
                if s != "eliminate"
                    && (!candidates.contains(&UnitId::new(s))
                        || !substitutes(content, state, named, row).contains(&UnitId::new(s))
                        || !selected.insert(s.to_owned()))
                {
                    return Err(illegal("substitute is ineligible or selected twice"));
                }
            }
            Job::Transport { assets, .. } => {
                let options = transport_options(assets);
                let mut seen = BTreeSet::new();
                for v in a["priority"]
                    .as_array()
                    .ok_or_else(|| illegal("rank empty truck sources"))?
                {
                    let k = v
                        .as_str()
                        .ok_or_else(|| illegal("rank listed holding/type pairs"))?;
                    if !options.iter().any(|s| s == k) || !seen.insert(k) {
                        return Err(illegal(
                            "withdrawal priority repeats or names an unlisted source",
                        ));
                    }
                }
            }
            Job::Air {
                quotas, remaining, ..
            } => {
                let q = a["quota"]
                    .as_str()
                    .and_then(|s| s.parse::<i32>().ok())
                    .filter(|q| quotas.contains(q))
                    .ok_or_else(|| illegal("choose a balanced weekly quota"))?;
                let mut n = 0_i64;
                for (plane, max) in remaining {
                    let v = a["planes"][plane]
                        .as_i64()
                        .filter(|n| *n >= 0 && *n <= i64::from(*max))
                        .ok_or_else(|| illegal("aircraft allocation exceeds its interval"))?;
                    n += v;
                }
                if n > i64::from(q) || (state.cursor.op_stage == Some(3) && n != i64::from(q)) {
                    return Err(illegal(
                        "aircraft allocation must fit the weekly quota and finish it by the last Operations Stage",
                    ));
                }
            }
        }
    }
    state
        .land
        .arrivals
        .window
        .as_mut()
        .unwrap()
        .answers
        .insert(pending.seat, action.clone());
    Ok("Private arrival orders recorded for shared closure.".into())
}

fn value_for<'a>(w: &'a Window, role: Role, side: Side, key: &str) -> Result<&'a Value, Rejection> {
    w.answers
        .get(&SeatId::new(side, role))
        .and_then(|a| a.get(key))
        .ok_or_else(|| illegal("complete arrival order is missing"))
}
fn legacy_action(
    content: &CnaContent,
    state: &State,
    w: &mut Window,
    p: &Pending,
    task: &Task,
) -> Result<Value, Rejection> {
    let side = p.seat.side;
    match task {
        Task::Place { unit, .. } => {
            Ok(value_for(w, Role::Commander, side, &format!("place:{unit}"))?.clone())
        }
        Task::Trucks { row } => {
            let b = &state.land.arrivals.batches[row];
            if b.alone || b.units.is_empty() {
                return Ok(value_for(w, Role::Logistics, side, &format!("pool:{row}"))?.clone());
            }
            let key = format!("trucks:{row}");
            let a = w
                .answers
                .get_mut(&SeatId::new(side, Role::Logistics))
                .unwrap()
                .get_mut(&key)
                .ok_or_else(|| illegal("truck order missing"))?;
            for unit in &b.units {
                let t = counts(&a[unit.as_str()])?;
                if t.total() > 0 {
                    a[unit.as_str()] = json!({"light":0,"medium":0,"heavy":0});
                    return Ok(
                        json!({"unit":unit,"light":t.light,"medium":t.medium,"heavy":t.heavy}),
                    );
                }
            }
            Err(illegal("arrival truck plan does not conserve the row"))
        }
        Task::Substitute { row, named } => Ok(value_for(
            w,
            Role::Commander,
            side,
            &format!("substitute:{row}:{named}"),
        )?
        .clone()),
        Task::Transport { row, source } => {
            let assets = empty_trucks(content, state);
            let a = w
                .answers
                .get(&SeatId::new(side, Role::Logistics))
                .and_then(|a| a.get(format!("transport:{row}")))
                .cloned()
                .unwrap_or(json!({"priority":[]}));
            let mut priorities: Vec<_> = a["priority"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            for key in transport_options(&assets) {
                if !priorities.contains(&key) {
                    priorities.push(key);
                }
            }
            for key in priorities {
                let Some((asset, kind)) = key.rsplit_once('|') else {
                    continue;
                };
                if source.as_ref().is_some_and(|s| s != asset) {
                    continue;
                }
                let Some(t) = assets.get(asset) else {
                    continue;
                };
                let available = match kind {
                    "light" => t.light,
                    "medium" => t.medium,
                    "heavy" => t.heavy,
                    _ => 0,
                };
                if available <= 0 {
                    continue;
                }
                if source.is_none() {
                    return Ok(json!(asset));
                }
                let weights = weights(content).map_err(Rejection::Engine)?;
                let weight = match kind {
                    "light" => weights.light,
                    "medium" => weights.medium,
                    "heavy" => weights.heavy,
                    _ => unreachable!(),
                };
                let needed = state.land.arrivals.withdrawals[row].needed_halves;
                let n = ((needed + i64::from(weight) - 1) / i64::from(weight))
                    .min(i64::from(available));
                let mut t = Trucks::default();
                match kind {
                    "light" => t.light = n as i32,
                    "medium" => t.medium = n as i32,
                    "heavy" => t.heavy = n as i32,
                    _ => unreachable!(),
                };
                return Ok(json!({"light":t.light,"medium":t.medium,"heavy":t.heavy}));
            }
            Err(illegal("no eligible physical withdrawal trucks"))
        }
        Task::Air { row, quota } => {
            let key = format!("air:{row}");
            let a = w
                .answers
                .get_mut(&SeatId::new(side, Role::Air))
                .unwrap()
                .get_mut(&key)
                .ok_or_else(|| illegal("aircraft order missing"))?;
            if *quota {
                return Ok(a["quota"].clone());
            }
            for (plane, max) in &state.land.arrivals.air_remaining[row] {
                let n = a["planes"][plane]
                    .as_i64()
                    .unwrap_or(0)
                    .min(i64::from(*max))
                    .min(i64::from(state.land.arrivals.air_week[row].1));
                if n > 0 {
                    a["planes"][plane] = json!(a["planes"][plane].as_i64().unwrap() - n);
                    return Ok(json!(format!("{plane}:{n}")));
                }
            }
            Ok(json!("defer"))
        }
    }
}
/// All six answers close before any placement, removal or aircraft count is applied.
/// Internal serial helpers are pure implementation details: their requests/counters never escape.
/// Cases: land:3.6, land:20.12, land:20.83, airlog:34.84
pub(super) fn finish(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<bool, EngineError> {
    let Some(w) = state.land.arrivals.window.as_ref() else {
        return Ok(false);
    };
    if w.stage != stage(state) {
        return Err(invariant("arrival window stage mismatch"));
    }
    if w.resolved {
        return Ok(true);
    }
    if seats().any(|s| !w.answers.contains_key(&s)) {
        return Ok(false);
    }
    let mut w = w.clone();
    let mut draft = state.clone();
    let decisions = draft.decisions.clone();
    let mut events = Vec::new();
    let mut local = Cx {
        rng: cx.rng,
        events: &mut events,
    };
    // Restore the source weekly ledger; requests are answered only inside this closure.
    air_start(content, &mut draft, &mut local)?;
    for _ in 0..10000 {
        if let Some(p) = draft
            .decisions
            .pending
            .iter()
            .find(|p| draft.land.arrivals.tasks.contains_key(&p.id))
            .cloned()
        {
            let task = draft.land.arrivals.tasks[&p.id].clone();
            let a = legacy_action(content, &draft, &mut w, &p, &task).map_err(|r| {
                invariant(format!(
                    "prepared arrival order could not be resolved: {r:?}"
                ))
            })?;
            p.space.check(&a).map_err(invariant)?;
            super::answer(content, &mut draft, &p, &a, strict, &mut local)
                .map_err(|r| invariant(format!("prepared arrival order failed closure: {r:?}")))?;
            draft.decisions.pending.retain(|q| q.id != p.id);
            continue;
        }
        advance_arrivals(content, &mut draft, strict, &mut local)?;
        if !draft.land.arrivals.tasks.is_empty() {
            continue;
        }
        prepare_withdrawals(content, &mut draft, strict, &mut local)?;
        if !draft.land.arrivals.tasks.is_empty() {
            continue;
        }
        report_air_withdrawals(content, &mut draft, strict, &mut local)?;
        draft.decisions = decisions;
        // Preserve original private orders for deterministic checkpoint/replay, not consumed cursors.
        draft.land.arrivals.window.as_mut().unwrap().resolved = true;
        *state = draft;
        for event in events {
            if !matches!(event.event, GameEvent::DecisionOpened { .. }) {
                cx.emit(event);
            }
        }
        return Ok(true);
    }
    Err(invariant(
        "arrival closure did not consume its finite prepared plan",
    ))
}
/// Deterministic source-conserving orders, without inspecting enemy hidden composition.
/// Cases: land:20.12, land:4.43, land:20.85, airlog:34.84
pub fn baseline(content: &CnaContent, state: &State, request: &DecisionRequest) -> Option<Value> {
    if request.kind != BATCH {
        return None;
    }
    let w = state.land.arrivals.window.as_ref()?;
    let jobs = w.jobs.get(&request.seat)?;
    if jobs.is_empty() {
        return Some(Value::Null);
    }
    let mut draft = state.clone();
    let mut a = serde_json::Map::new();
    let mut substitutes_used = BTreeSet::new();
    for j in jobs {
        let v = match j {
            Job::Place {
                unit, destinations, ..
            } => {
                if let Some(l) = destinations
                    .iter()
                    .find(|l| valid_destination(content, &draft, unit, l, false).is_ok())
                {
                    draft.land.units.get_mut(unit)?.location = l.clone();
                    json!(crate::setup::placement::destination_id(l)?)
                } else {
                    json!("await_capacity")
                }
            }
            Job::Pool { destinations, .. } => json!(crate::setup::placement::destination_id(
                destinations.first()?
            )?),
            Job::Trucks {
                units, available, ..
            } => {
                let mut out = serde_json::Map::new();
                for (i, u) in units.iter().enumerate() {
                    let t = if i == 0 {
                        *available
                    } else {
                        Trucks::default()
                    };
                    out.insert(
                        u.to_string(),
                        json!({"light":t.light,"medium":t.medium,"heavy":t.heavy}),
                    );
                }
                Value::Object(out)
            }
            Job::Substitute { candidates, .. } => json!(
                candidates
                    .iter()
                    .find(|u| !substitutes_used.contains(*u))
                    .map(|u| {
                        substitutes_used.insert(u.clone());
                        u.to_string()
                    })
                    .unwrap_or_else(|| "eliminate".into())
            ),
            Job::Transport { assets, .. } => json!({"priority":transport_options(assets)}),
            Job::Air {
                quotas, remaining, ..
            } => {
                let q = *quotas.first()?;
                let mut left = q;
                let mut out = serde_json::Map::new();
                for (p, n) in remaining {
                    let n = (*n).min(left);
                    left -= n;
                    out.insert(p.clone(), json!(n));
                }
                json!({"quota":q.to_string(),"planes":out})
            }
        };
        a.insert(j.key(), v);
    }
    Some(Value::Object(a))
}
