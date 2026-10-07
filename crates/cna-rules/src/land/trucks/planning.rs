//! Finite own-holdings witnesses for reaction CPA options; no opposing facts are queried.
use super::*;
use cna_tables::airlog::{supply::SupplyType, trucks::TruckType};
const KINDS: [TruckType; 3] = [TruckType::Light, TruckType::Medium, TruckType::Heavy];
const GOODS: [SupplyType; 4] = [
    SupplyType::Ammo,
    SupplyType::Fuel,
    SupplyType::Stores,
    SupplyType::Water,
];
fn ns(t: Trucks) -> [i32; 3] {
    [t.light, t.medium, t.heavy]
}
fn trucks(n: [i32; 3]) -> Trucks {
    Trucks {
        light: n[0],
        medium: n[1],
        heavy: n[2],
    }
}
fn points(s: Supplies) -> [i32; 4] {
    [s.ammo, s.fuel, s.stores, s.water]
}
fn goods(n: [i32; 4]) -> Supplies {
    Supplies {
        ammo: n[0],
        fuel: n[1],
        stores: n[2],
        water: n[3],
    }
}
fn cargo_mut(p: &mut CargoPacking, k: usize) -> &mut Supplies {
    match k {
        0 => &mut p.light,
        1 => &mut p.medium,
        _ => &mut p.heavy,
    }
}
struct Pack {
    demand: [i32; 4],
    cost: Vec<[i64; 4]>,
    allocation: Vec<[i32; 4]>,
    failed: BTreeSet<(usize, usize, i32, Vec<i64>)>,
}
impl Pack {
    fn place(&mut self, r: usize, b: usize, need: i32, rooms: Vec<i64>) -> bool {
        if r == 4 {
            return true;
        }
        if b == rooms.len() {
            return need == 0
                && self.place(
                    r + 1,
                    0,
                    self.demand.get(r + 1).copied().unwrap_or(0),
                    rooms,
                );
        }
        let key = (r, b, need, rooms.clone());
        if self.failed.contains(&key) {
            return false;
        }
        if (r + 1..4).any(|t| {
            rooms
                .iter()
                .enumerate()
                .map(|(i, n)| n / self.cost[i][t])
                .sum::<i64>()
                < i64::from(self.demand[t])
        }) {
            self.failed.insert(key);
            return false;
        }
        let later = (b + 1..rooms.len())
            .map(|i| rooms[i] / self.cost[i][r])
            .sum::<i64>();
        let lo = (i64::from(need) - later).max(0);
        let hi = i64::from(need).min(rooms[b] / self.cost[b][r]);
        for n in (lo..=hi).rev() {
            let mut next = rooms.clone();
            next[b] -= n * self.cost[b][r];
            self.allocation[b][r] = n as i32;
            if self.place(r, b + 1, need - n as i32, next) {
                return true;
            }
        }
        self.allocation[b][r] = 0;
        self.failed.insert(key);
        false
    }
}
fn pack(
    c: &CnaContent,
    attached: &[[i32; 3]],
    transport: &[[i32; 3]],
    stock: Supplies,
) -> Option<Vec<CargoPacking>> {
    let demand = points(stock);
    if demand.iter().any(|n| *n < 0) {
        return None;
    }
    let mut rooms = vec![];
    let mut cost = vec![];
    for (a, t) in attached.iter().zip(transport) {
        for k in 0..3 {
            let n = a[k].checked_sub(t[k]).filter(|n| *n >= 0)?;
            let chart = c.tables.airlog.truck_characteristics.truck(KINDS[k]);
            let den = GOODS.iter().try_fold(1i64, |d, ty| {
                d.checked_mul(i64::from(chart.supply_capacity(*ty)))
            })?;
            if den <= 0 {
                return None;
            }
            rooms.push(i64::from(n).checked_mul(den)?);
            cost.push(GOODS.map(|ty| den / i64::from(chart.supply_capacity(ty))));
        }
    }
    let mut p = Pack {
        demand,
        cost,
        allocation: vec![[0; 4]; rooms.len()],
        failed: BTreeSet::new(),
    };
    if !p.place(0, 0, demand[0], rooms) {
        return None;
    }
    let mut result = vec![CargoPacking::default(); attached.len()];
    for (i, n) in p.allocation.into_iter().enumerate() {
        *cargo_mut(&mut result[i / 3], i % 3) = goods(n);
    }
    Some(result)
}
fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: detail.into(),
    }
}
fn choices(
    s: &State,
    id: &UnitId,
    counts: [i32; 3],
    strict: bool,
) -> Result<Option<Vec<FuelCohortSelection>>, EngineError> {
    let mut need = counts;
    let mut choices = vec![];
    let Some(groups) = super::super::movement::feasible(
        logistics::segment_fuel_cohorts(s, id).map_err(|e| supply(e, strict)),
    )?
    else {
        return Ok(None);
    };
    for g in groups {
        if g.count <= 0 {
            return Err(invariant(
                "reaction division has invalid physical cohort count",
            ));
        }
        let k = match g.kind {
            FuelTruckKind::Light => 0,
            FuelTruckKind::Medium => 1,
            FuelTruckKind::Heavy => 2,
        };
        let n = need[k].min(g.count);
        if n > 0 {
            choices.push(FuelCohortSelection { id: g.id, count: n });
            need[k] -= n;
        }
    }
    Ok((need == [0; 3]).then_some(choices))
}
fn body_capacity(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    strict: bool,
) -> Result<Option<i32>, EngineError> {
    let mut draft = s.clone();
    draft
        .land
        .units
        .get_mut(id)
        .ok_or_else(|| invariant("reaction division is missing a trusted family member"))?
        .trucks = Trucks::default();
    super::super::movement::feasible(
        logistics::fuel_capacity(c, &draft, id)
            .map(|n| n.get())
            .map_err(|e| supply(e, strict)),
    )
}

fn tank_capacity(c: &CnaContent, n: [i32; 3]) -> Option<i32> {
    (0..3).try_fold(0i32, |sum, k| {
        sum.checked_add(
            n[k].checked_mul(
                c.tables
                    .airlog
                    .truck_characteristics
                    .truck(KINDS[k])
                    .fuel_capacity_points,
            )?
            .checked_mul(10)?,
        )
    })
}
fn append(
    c: &CnaContent,
    s: &mut State,
    d: &mut Division,
    t: Transfer,
    strict: bool,
) -> Result<Option<()>, EngineError> {
    if super::super::movement::feasible(apply_transfer(c, s, &t, strict))?.is_none() {
        return Ok(None);
    }
    d.transfers.push(t);
    Ok(Some(()))
}
/// Consolidate parent-owned trucks, then divide the exact physical cohorts and cargo.
/// Intermediate concentrations exist only inside the transaction; final cohesion retention,
/// capacity and conservation are checked by the same validator used for an answer.
/// Cases: land:8.56, land:8.95, land:8.96, land:8.97, airlog:54.2
fn witness(
    c: &CnaContent,
    s: &State,
    reactor: &UnitId,
    ids: &[UnitId],
    a: &[[i32; 3]],
    t: &[[i32; 3]],
    strict: bool,
) -> Result<Option<Division>, EngineError> {
    let Some((root, packing)) = (|| {
        let root = ids
            .iter()
            .position(|id| ownership::parent_for_unit(c, s, id).is_none_or(|p| !ids.contains(p)))?;
        let stock = ids.iter().try_fold(Supplies::default(), |sum, id| {
            let mut sum = points(sum);
            let n = points(
                s.logistics
                    .unit_supply
                    .get(id)
                    .cloned()
                    .unwrap_or_default()
                    .carried,
            );
            for k in 0..4 {
                sum[k] = sum[k].checked_add(n[k])?;
            }
            Some(goods(sum))
        })?;
        let packing = pack(c, a, t, stock)?;
        Some((root, packing))
    })() else {
        return Ok(None);
    };
    let mut draft = s.clone();
    let mut d = Division::default();
    for (i, id) in ids.iter().enumerate().filter(|(i, _)| *i != root) {
        let n = ns(draft.land.units[id].trucks);
        if n == [0; 3] {
            continue;
        }
        let source = draft
            .logistics
            .unit_supply
            .get(id)
            .cloned()
            .unwrap_or_default();
        let Some(mut loaded) = pack(c, &[n], &[[0; 3]], source.carried) else {
            return Ok(None);
        };
        let loaded = loaded.remove(0);
        let Some(body) = body_capacity(c, &draft, id, strict)? else {
            return Ok(None);
        };
        let fuel = (source.tank_fuel.get() - body).max(0);
        let Some(selected) = choices(&draft, id, n, strict)? else {
            return Ok(None);
        };
        let t = Transfer {
            from: id.clone(),
            to: ids[root].clone(),
            cohorts: selected,
            cargo: loaded,
            tank_fuel_tenths: fuel,
            activity_water_points: source.activity_water.get(),
        };
        if append(c, &mut draft, &mut d, t, strict)?.is_none() {
            return Ok(None);
        }
        debug_assert_eq!(ns(draft.land.units[&ids[i]].trucks), [0; 3]);
    }
    // Return vehicle fuel with the physical trucks, preserving each body's original fuel.
    let Some(root_body) = body_capacity(c, &draft, &ids[root], strict)? else {
        return Ok(None);
    };
    let mut spare_fuel = (draft
        .logistics
        .unit_supply
        .get(&ids[root])
        .cloned()
        .unwrap_or_default()
        .tank_fuel
        .get()
        - root_body)
        .max(0);
    for (i, id) in ids.iter().enumerate().filter(|(i, _)| *i != root) {
        if a[i] == [0; 3] {
            continue;
        }
        let fuel = spare_fuel.min(
            tank_capacity(c, a[i])
                .ok_or_else(|| invariant("reaction division tank capacity overflow"))?,
        );
        spare_fuel -= fuel;
        let water = draft
            .logistics
            .unit_supply
            .get(&ids[root])
            .cloned()
            .unwrap_or_default()
            .activity_water
            .get();
        let required = a[i]
            .into_iter()
            .try_fold(0i32, |sum, n| sum.checked_add(n))
            .ok_or_else(|| invariant("reaction division water demand overflow"))?;
        let Some(selected) = choices(&draft, &ids[root], a[i], strict)? else {
            return Ok(None);
        };
        let t = Transfer {
            from: ids[root].clone(),
            to: id.clone(),
            cohorts: selected,
            cargo: packing[i].clone(),
            tank_fuel_tenths: fuel,
            activity_water_points: water.min(required),
        };
        if append(c, &mut draft, &mut d, t, strict)?.is_none() {
            return Ok(None);
        }
    }
    d.allocations = ids
        .iter()
        .enumerate()
        .map(|(i, id)| Allocation {
            unit: id.clone(),
            transport: trucks(t[i]),
            packing: packing[i].clone(),
        })
        .collect();
    if super::super::movement::feasible(preview_reaction_division(c, s, reactor, &d, strict))?
        .is_none()
    {
        return Ok(None);
    }
    Ok(Some(d))
}
struct Search<'a> {
    c: &'a CnaContent,
    s: &'a State,
    reactor: &'a UnitId,
    ids: Vec<UnitId>,
    moving: BTreeSet<UnitId>,
    root: usize,
    strict: bool,
    target: i32,
    transport: Vec<[i32; 3]>,
    answer: Option<Division>,
}
impl Search<'_> {
    fn retained(
        &mut self,
        i: usize,
        remaining: [i32; 3],
        mut a: Vec<[i32; 3]>,
    ) -> Result<(), EngineError> {
        if self.answer.is_some() {
            return Ok(());
        }
        if i == self.ids.len() {
            for k in 0..3 {
                a[self.root][k] += remaining[k];
            }
            self.answer = witness(
                self.c,
                self.s,
                self.reactor,
                &self.ids,
                &a,
                &self.transport,
                self.strict,
            )?;
            return Ok(());
        }
        let u = &self.s.land.units[&self.ids[i]];
        if u.cohesion_quarters <= -20 && u.trucks.total() > 0 && a[i] == [0; 3] {
            for k in 0..3 {
                if remaining[k] > 0 {
                    let mut next = remaining;
                    next[k] -= 1;
                    let mut n = a.clone();
                    n[i][k] += 1;
                    self.retained(i + 1, next, n)?;
                }
            }
        } else {
            self.retained(i + 1, remaining, a)?;
        }
        Ok(())
    }
    fn transport(&mut self, i: usize, remaining: [i32; 3]) -> Result<(), EngineError> {
        if self.answer.is_some() {
            return Ok(());
        }
        if i == self.ids.len() {
            self.retained(0, remaining, self.transport.clone())?;
            return Ok(());
        }
        let id = &self.ids[i];
        if !self.moving.contains(id) {
            self.transport(i + 1, remaining)?;
            return Ok(());
        }
        let mut base = self.s.clone();
        base.land.units.get_mut(id).unwrap().transport_trucks = Trucks::default();
        if formation::individual_allowance(self.c, &base, id).is_some_and(|a| a.cpa >= self.target)
        {
            self.transport(i + 1, remaining)?;
            return Ok(());
        }
        let Some(class) = formation::class(self.c, id) else {
            return Ok(());
        };
        let need = formation::strength(self.c, self.s, id)
            .checked_mul(2)
            .ok_or_else(|| invariant("reaction division transport demand overflow"))?;
        if need <= 0 {
            return Ok(());
        }
        let capacities = KINDS.map(|kind| {
            let chart = self.c.tables.airlog.truck_characteristics.truck(kind);
            let (cap, cpa) = match class.unit_type.as_str() {
                "infantry" => (chart.capacity_inf_toe_halves, Some(chart.cpa_inf)),
                "anti_air" => (chart.capacity_aa_toe * 2, chart.cpa_guns),
                "artillery" | "anti_tank" => {
                    (chart.capacity_arty_toe.unwrap_or(0) * 2, chart.cpa_guns)
                }
                _ => (0, None),
            };
            if cap > 0
                && cpa.is_some_and(|cpa| {
                    super::super::reserve::adjust_allowance(
                        &self.s.land.units[id],
                        super::super::capability::Allowance {
                            cpa,
                            motorized: true,
                        },
                    )
                    .cpa >= self.target
                })
            {
                cap
            } else {
                0
            }
        });
        let bound = |k: usize| {
            if capacities[k] > 0 {
                remaining[k].min((need + capacities[k] - 1) / capacities[k])
            } else {
                0
            }
        };
        for l in 0..=bound(0) {
            for m in 0..=bound(1) {
                for h in 0..=bound(2) {
                    let n = [l, m, h];
                    let cap = (0..3).map(|k| n[k] * capacities[k]).sum::<i32>();
                    if cap < need || (0..3).any(|k| n[k] > 0 && cap - capacities[k] >= need) {
                        continue;
                    }
                    self.transport[i] = n;
                    self.transport(i + 1, std::array::from_fn(|k| remaining[k] - n[k]))?;
                    if self.answer.is_some() {
                        return Ok(());
                    }
                }
            }
        }
        self.transport[i] = [0; 3];
        Ok(())
    }
}
/// Every offered rating has a complete legal witness using only this family's holdings.
/// The search is bounded by TOE transport needs and exact stock/capacity; it never tests chart
/// ratings the component cannot physically reach and never reads opposing units or pins.
/// Cases: land:8.56, land:8.91, land:8.92, land:8.95, land:8.96, land:8.97
pub fn reachable_divisions(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    strict: bool,
) -> Result<BTreeMap<i32, Option<Division>>, EngineError> {
    let mut out = BTreeMap::new();
    if let Some(a) = formation::allowance(c, s, id) {
        out.insert(a.cpa, None);
    }
    if ownership::parent_for_unit(c, s, id).is_none() {
        return Ok(out);
    }
    let ids = family(c, s, id);
    let Some(root) = ids
        .iter()
        .position(|id| ownership::parent_for_unit(c, s, id).is_none_or(|p| !ids.contains(p)))
    else {
        return Ok(out);
    };
    let Some(available) = ids.iter().try_fold([0i32; 3], |mut n, id| {
        for (k, count) in ns(s.land.units[id].trucks).into_iter().enumerate() {
            if count < 0 {
                return None;
            }
            n[k] = n[k].checked_add(count)?;
        }
        Some(n)
    }) else {
        return Err(invariant(
            "reaction division family truck inventory is invalid or overflows",
        ));
    };
    if available == [0; 3] {
        return Ok(out);
    }
    let moving: BTreeSet<_> = formation::members(c, s, id).into_iter().collect();
    let mut ratings = BTreeSet::new();
    for member in &moving {
        let Some(class) = formation::class(c, member) else {
            continue;
        };
        for kind in KINDS {
            let chart = c.tables.airlog.truck_characteristics.truck(kind);
            let raw = match class.unit_type.as_str() {
                "infantry" => Some(chart.cpa_inf),
                "anti_air" | "artillery" | "anti_tank" => chart.cpa_guns,
                _ => None,
            };
            if let Some(cpa) = raw {
                ratings.insert(
                    super::super::reserve::adjust_allowance(
                        &s.land.units[member],
                        super::super::capability::Allowance {
                            cpa,
                            motorized: true,
                        },
                    )
                    .cpa,
                );
            }
        }
    }
    for target in ratings {
        if out.contains_key(&target) {
            continue;
        }
        let mut search = Search {
            c,
            s,
            reactor: id,
            transport: vec![[0; 3]; ids.len()],
            ids: ids.clone(),
            moving: moving.clone(),
            root,
            strict,
            target,
            answer: None,
        };
        search.transport(0, available)?;
        if let Some(d) = search.answer {
            let Some(draft) =
                super::super::movement::feasible(preview_reaction_division(c, s, id, &d, strict))?
            else {
                continue;
            };
            if let Some(a) = formation::allowance(c, &draft, id) {
                out.entry(a.cpa).or_insert(Some(d));
            }
        }
    }
    Ok(out)
}
