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
fn invariant(detail: &str) -> Rejection {
    formation::query_invariant(detail)
}
// This is a trusted history/capacity read, not validation of an actor's quantities.
// Preserve existing profile-specific unknown-fuel policy; never discard its Engine variant.
fn trusted_supply(error: SupplyError, strict: bool) -> Rejection {
    match error {
        SupplyError::Invalid => invariant("invalid trusted truck planning history or capacity"),
        other => supply(other, strict),
    }
}
fn checked_pack(
    c: &CnaContent,
    attached: &[[i32; 3]],
    transport: &[[i32; 3]],
    stock: Supplies,
) -> Result<Vec<CargoPacking>, Rejection> {
    if attached.len() != transport.len() || points(stock).iter().any(|n| *n < 0) {
        return Err(invariant("invalid trusted packing shape or stock"));
    }
    let mut total_room = 0i64;
    for (a, t) in attached.iter().zip(transport) {
        for k in 0..3 {
            if a[k] < 0 || t[k] < 0 {
                return Err(invariant(
                    "negative trusted/generated packing truck quantity",
                ));
            }
            if a[k] < t[k] {
                return Err(illegal("no feasible candidate packing"));
            }
            let chart = c.tables.airlog.truck_characteristics.truck(KINDS[k]);
            let mut den = 1i64;
            for ty in GOODS {
                let cap = chart.supply_capacity(ty);
                if cap <= 0 {
                    return Err(invariant("nonpositive validated supply capacity"));
                }
                den = den
                    .checked_mul(i64::from(cap))
                    .ok_or_else(|| invariant("packing denominator arithmetic overflow"))?;
            }
            let room = i64::from(a[k] - t[k])
                .checked_mul(den)
                .ok_or_else(|| invariant("packing room arithmetic overflow"))?;
            total_room = total_room
                .checked_add(room)
                .ok_or_else(|| invariant("packing aggregate room arithmetic overflow"))?;
        }
    }
    // Every Option exit except genuine packing infeasibility was checked above.
    pack(c, attached, transport, stock).ok_or_else(|| illegal("no feasible candidate packing"))
}
fn choices(
    s: &State,
    id: &UnitId,
    counts: [i32; 3],
    strict: bool,
) -> Result<Vec<FuelCohortSelection>, Rejection> {
    let mut need = counts;
    if need.iter().any(|n| *n < 0) {
        return Err(invariant("negative generated cohort selection"));
    }
    let mut choices = vec![];
    for g in logistics::segment_fuel_cohorts(s, id).map_err(|e| trusted_supply(e, strict))? {
        if g.count < 0 {
            return Err(invariant("negative trusted fuel cohort"));
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
    if need != [0; 3] {
        return Err(invariant(
            "trusted fuel cohorts do not cover the generated physical selection",
        ));
    }
    Ok(choices)
}
fn body_capacity(c: &CnaContent, s: &State, id: &UnitId, strict: bool) -> Result<i32, Rejection> {
    formation::query_individual_allowance(c, s, id, formation::QueryAssignment::Trusted)?;
    let mut draft = s.clone();
    let unit = draft
        .land
        .units
        .get_mut(id)
        .ok_or_else(|| invariant("truck capacity references a missing trusted unit"))?;
    if matches!(
        &unit.toe,
        Some(cna_content::units::Toe::Under { .. } | cna_content::units::Toe::Over { .. })
    ) && formation::query_class(c, id)?.max_toe.is_none()
    {
        // Preserve the missing source maximum before fuel_capacity can collapse it to Invalid.
        formation::query_strength(c, s, id)?;
    }
    unit.trucks = Trucks::default();
    logistics::fuel_capacity(c, &draft, id)
        .map(|n| n.get())
        .map_err(|e| trusted_supply(e, strict))
}
fn tank_capacity(c: &CnaContent, n: [i32; 3]) -> Result<i32, Rejection> {
    (0..3).try_fold(0i32, |sum, k| {
        let cap = c
            .tables
            .airlog
            .truck_characteristics
            .truck(KINDS[k])
            .fuel_capacity_points;
        if n[k] < 0 || cap < 0 {
            return Err(invariant("negative generated tank capacity input"));
        }
        n[k].checked_mul(cap)
            .and_then(|v| v.checked_mul(10))
            .and_then(|v| sum.checked_add(v))
            .ok_or_else(|| invariant("tank capacity arithmetic overflow"))
    })
}
fn append(
    c: &CnaContent,
    s: &mut State,
    d: &mut Division,
    t: Transfer,
    strict: bool,
) -> Result<(), Rejection> {
    apply_transfer(c, s, &t, strict)?;
    d.transfers.push(t);
    Ok(())
}
/// Only genuine candidate infeasibility becomes None; a typed engine failure survives.
/// Cases: land:8.56, land:8.88, land:8.95, land:8.96, land:8.97, airlog:54.2
fn witness(
    c: &CnaContent,
    s: &State,
    reactor: &UnitId,
    ids: &[UnitId],
    a: &[[i32; 3]],
    t: &[[i32; 3]],
    strict: bool,
) -> Result<Option<Division>, Rejection> {
    super::super::movement::query_candidate((|| {
        let root = ids
            .iter()
            .position(|id| ownership::parent_for_unit(c, s, id).is_none_or(|p| !ids.contains(p)))
            .ok_or_else(|| invariant("trusted truck family has no root"))?;
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
                if n[k] < 0 {
                    return Err(invariant("negative trusted family cargo"));
                }
                sum[k] = sum[k]
                    .checked_add(n[k])
                    .ok_or_else(|| invariant("family cargo arithmetic overflow"))?;
            }
            Ok(goods(sum))
        })?;
        let packing = checked_pack(c, a, t, stock)?;
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
            let loaded = checked_pack(c, &[n], &[[0; 3]], source.carried)?.remove(0);
            let body = body_capacity(c, &draft, id, strict)?;
            let fuel = source
                .tank_fuel
                .get()
                .checked_sub(body)
                .ok_or_else(|| invariant("body tank arithmetic overflow"))?
                .max(0);
            let transfer = Transfer {
                from: id.clone(),
                to: ids[root].clone(),
                cohorts: choices(&draft, id, n, strict)?,
                cargo: loaded,
                tank_fuel_tenths: fuel,
                activity_water_points: source.activity_water.get(),
            };
            append(c, &mut draft, &mut d, transfer, strict)?;
            debug_assert_eq!(ns(draft.land.units[&ids[i]].trucks), [0; 3]);
        }
        let root_body = body_capacity(c, &draft, &ids[root], strict)?;
        let mut spare_fuel = draft
            .logistics
            .unit_supply
            .get(&ids[root])
            .cloned()
            .unwrap_or_default()
            .tank_fuel
            .get()
            .checked_sub(root_body)
            .ok_or_else(|| invariant("root tank arithmetic overflow"))?
            .max(0);
        for (i, id) in ids.iter().enumerate().filter(|(i, _)| *i != root) {
            if a[i] == [0; 3] {
                continue;
            }
            let fuel = spare_fuel.min(tank_capacity(c, a[i])?);
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
                .ok_or_else(|| invariant("truck water quantity arithmetic overflow"))?;
            let transfer = Transfer {
                from: ids[root].clone(),
                to: id.clone(),
                cohorts: choices(&draft, &ids[root], a[i], strict)?,
                cargo: packing[i].clone(),
                tank_fuel_tenths: fuel,
                activity_water_points: water.min(required),
            };
            append(c, &mut draft, &mut d, transfer, strict)?;
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
        preview_reaction_division(c, s, reactor, &d, strict)?;
        Ok(d)
    })())
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
    ) -> Result<(), Rejection> {
        if self.answer.is_some() {
            return Ok(());
        }
        if i == self.ids.len() {
            for k in 0..3 {
                a[self.root][k] = a[self.root][k]
                    .checked_add(remaining[k])
                    .ok_or_else(|| invariant("retained truck arithmetic overflow"))?;
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
    fn transport(&mut self, i: usize, remaining: [i32; 3]) -> Result<(), Rejection> {
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
        if formation::query_individual_allowance(
            self.c,
            &base,
            id,
            formation::QueryAssignment::Trusted,
        )?
        .cpa >= self.target
        {
            self.transport(i + 1, remaining)?;
            return Ok(());
        }
        let class = formation::query_class(self.c, id)?;
        let need = formation::query_strength(self.c, self.s, id)?
            .checked_mul(2)
            .ok_or_else(|| invariant("transport need arithmetic overflow"))?;
        if need <= 0 {
            return Ok(());
        }
        let mut capacities = [0i32; 3];
        if matches!(
            class.unit_type.as_str(),
            "infantry" | "anti_air" | "artillery" | "anti_tank"
        ) {
            for (k, kind) in KINDS.into_iter().enumerate() {
                if let Some((cap, cpa)) = formation::query_transport_cell(self.c, class, kind)?
                    && cap > 0
                    && super::super::reserve::adjust_allowance(
                        &self.s.land.units[id],
                        super::super::capability::Allowance {
                            cpa,
                            motorized: true,
                        },
                    )
                    .cpa >= self.target
                {
                    capacities[k] = cap;
                }
            }
        }
        let bound = |k: usize| {
            if capacities[k] > 0 {
                remaining[k].min(
                    (i64::from(need) + i64::from(capacities[k]) - 1)
                        .div_euclid(i64::from(capacities[k])) as i32,
                )
            } else {
                0
            }
        };
        for l in 0..=bound(0) {
            for m in 0..=bound(1) {
                for h in 0..=bound(2) {
                    let n = [l, m, h];
                    let cap: i128 = (0..3)
                        .map(|k| i128::from(n[k]) * i128::from(capacities[k]))
                        .sum();
                    if cap < i128::from(need)
                        || (0..3).any(|k| {
                            n[k] > 0 && cap - i128::from(capacities[k]) >= i128::from(need)
                        })
                    {
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
/// Every offered rating has a complete witness; Engine failures never become search exhaustion.
/// Cases: land:8.56, land:8.88, land:8.91, land:8.92, land:8.95, land:8.96, land:8.97
pub fn reachable_divisions(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    strict: bool,
) -> Result<BTreeMap<i32, Option<Division>>, Rejection> {
    let mut out = BTreeMap::new();
    let a = formation::query_allowance(c, s, id)?;
    out.insert(a.cpa, None);
    if ownership::parent_for_unit(c, s, id).is_none() {
        return Ok(out);
    }
    let ids = family(c, s, id);
    let root = ids
        .iter()
        .position(|id| ownership::parent_for_unit(c, s, id).is_none_or(|p| !ids.contains(p)))
        .ok_or_else(|| invariant("trusted truck family has no root"))?;
    let available = ids.iter().try_fold([0i32; 3], |mut n, id| {
        let unit = s
            .land
            .units
            .get(id)
            .ok_or_else(|| invariant("missing trusted truck family member"))?;
        for (k, count) in ns(unit.trucks).into_iter().enumerate() {
            if count < 0 {
                return Err(invariant("negative trusted family truck holding"));
            }
            n[k] = n[k]
                .checked_add(count)
                .ok_or_else(|| invariant("family truck arithmetic overflow"))?;
        }
        Ok(n)
    })?;
    if available == [0; 3] {
        return Ok(out);
    }
    let moving: BTreeSet<_> = formation::members(c, s, id).into_iter().collect();
    let mut ratings = BTreeSet::new();
    for member in &moving {
        let class = formation::query_class(c, member)?;
        for kind in KINDS {
            let chart = c.tables.airlog.truck_characteristics.truck(kind);
            let raw = match class.unit_type.as_str() {
                "infantry" => Some(chart.cpa_inf),
                "anti_air" | "artillery" | "anti_tank" => chart.cpa_guns,
                _ => None,
            };
            if let Some(cpa) = raw {
                if cpa < 0 {
                    return Err(invariant("negative validated truck CPA"));
                }
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
            let Some(draft) = super::super::movement::query_candidate(preview_reaction_division(
                c, s, id, &d, strict,
            ))?
            else {
                continue;
            };
            let a = formation::query_allowance(c, &draft, id)?;
            out.entry(a.cpa).or_insert(Some(d));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod query_error_controls {
    use super::*;
    fn setup() -> (CnaContent, State, UnitId, UnitId) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        s.cursor.op_stage = Some(1);
        let from: UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
        let to: UnitId = "cw.2_nz_div.21st_nz_bn".into();
        for id in [&from, &to] {
            let u = s.land.units.get_mut(id).unwrap();
            u.location = crate::state::Location::Hex {
                hex: "C4022".into(),
            };
            u.trucks = Trucks::default();
            u.transport_trucks = Trucks::default();
            u.detached = true;
            u.attached_to = None;
        }
        s.land.units.get_mut(&from).unwrap().trucks.medium = 1;
        s.land.units.get_mut(&to).unwrap().detached = false;
        s.land.units.get_mut(&to).unwrap().attached_to = Some(from.clone());
        (c, s, from, to)
    }
    /// Cases: land:8.56, land:8.88, land:8.96
    #[test]
    fn strict_box_handling_survives_witness_and_enumeration_without_input_writes() {
        let (c, mut s, from, to) = setup();
        // One medium Truck Point must be enough to reach the transfer witness.
        s.land.units.get_mut(&to).unwrap().toe =
            Some(cna_content::units::Toe::Under { under: 1 });
        let strength = formation::query_strength(&c, &s, &to).unwrap();
        let medium = c.tables.airlog.truck_characteristics.truck(TruckType::Medium);
        assert_eq!(strength, 1);
        assert_eq!(medium.capacity_inf_toe_halves, strength * 2);
        assert_eq!(medium.cpa_inf, 20);
        let stage = logistics::water::WaterStage::current(&s);
        s.land.units.get_mut(&from).unwrap().box_handling =
            Some(logistics::box_handling::BoxHandling {
                stage,
                loaded: Supplies::default(),
                unloaded: Supplies::default(),
            });
        let before = serde_json::to_value(&s).unwrap();
        // Both are written as direct controls: witness with explicit allocation, then full search.
        let ids = vec![from.clone(), to.clone()];
        assert!(matches!(witness(&c, &s, &to, &ids, &[[0,0,0],[0,1,0]],
            &[[0,0,0],[0,0,0]], true),
            Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:8.88"));
        assert!(matches!(reachable_divisions(&c, &s, &to, true),
            Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:8.88"));
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }
    /// Cases: land:8.56, land:8.95, land:8.96, airlog:54.2
    #[test]
    fn infeasible_packing_is_illegal_candidate_but_trusted_negative_stock_is_engine_failure() {
        let (c, s, from, to) = setup();
        let ids = vec![from, to.clone()];
        assert!(matches!(
            checked_pack(
                &c,
                &[[0; 3]],
                &[[0; 3]],
                Supplies {
                    fuel: 1,
                    ..Supplies::default()
                }
            ),
            Err(Rejection::Illegal { .. })
        ));
        assert!(matches!(
            checked_pack(
                &c,
                &[[0; 3]],
                &[[0; 3]],
                Supplies {
                    fuel: -1,
                    ..Supplies::default()
                }
            ),
            Err(Rejection::Engine(EngineError::Invariant { .. }))
        ));
        assert!(matches!(
            witness(
                &c,
                &s,
                &to,
                &ids,
                &[[0, 0, 0], [0, 0, 0]],
                &[[0, 0, 0], [0, 1, 0]],
                true
            ),
            Ok(None)
        ));
    }
}
