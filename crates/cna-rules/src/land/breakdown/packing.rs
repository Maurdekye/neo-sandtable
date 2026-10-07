//! Finite integer fallback for mandatory cargo partitions; no stock is rounded away.
use crate::{CnaContent, logistics::capacity::CargoPacking};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_tables::airlog::trucks::TruckType;
use std::collections::BTreeSet;
const KINDS: [TruckType; 3] = [TruckType::Heavy, TruckType::Medium, TruckType::Light];
const TYPES: [cna_tables::airlog::supply::SupplyType; 4] = [
    cna_tables::airlog::supply::SupplyType::Ammo,
    cna_tables::airlog::supply::SupplyType::Stores,
    cna_tables::airlog::supply::SupplyType::Fuel,
    cna_tables::airlog::supply::SupplyType::Water,
];
fn counts(t: Trucks) -> [i32; 3] {
    [t.heavy, t.medium, t.light]
}
struct Search {
    demand: [i32; 4],
    cost: [[i64; 4]; 6],
    allocation: [[i32; 4]; 6],
    failed: BTreeSet<(usize, usize, i32, [i64; 6])>,
}
impl Search {
    fn place(&mut self, r: usize, b: usize, need: i32, rooms: [i64; 6]) -> bool {
        if r == 4 {
            return true;
        }
        if b == 6 {
            return need == 0
                && self.place(
                    r + 1,
                    0,
                    self.demand.get(r + 1).copied().unwrap_or(0),
                    rooms,
                );
        }
        let key = (r, b, need, rooms);
        if self.failed.contains(&key) {
            return false;
        }
        // This relaxation cannot prune a feasible integer packing.
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
        let later = (b + 1..6).map(|i| rooms[i] / self.cost[i][r]).sum::<i64>();
        let lo = (i64::from(need) - later).max(0);
        let hi = i64::from(need).min(rooms[b] / self.cost[b][r]);
        for n in (lo..=hi).rev() {
            let mut next = rooms;
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
/// Exhaust all stock- and chart-capacity-bounded integral assignments, memoizing failures.
/// Greedy helpers are the caller's fast path; this fallback never imposes an arbitrary cutoff.
/// Cases: land:21.43, airlog:53.11, airlog:54.2
pub(super) fn two(
    c: &CnaContent,
    a: Trucks,
    at: Trucks,
    b: Trucks,
    bt: Trucks,
    stock: Supplies,
) -> Option<(CargoPacking, CargoPacking)> {
    let available = [counts(a), counts(b)];
    let transport = [counts(at), counts(bt)];
    let mut rooms = [0; 6];
    let mut cost = [[0; 4]; 6];
    for i in 0..6 {
        let kind = KINDS[i % 3];
        let n = available[i / 3][i % 3]
            .checked_sub(transport[i / 3][i % 3])
            .filter(|n| *n >= 0)?;
        let chart = c.tables.airlog.truck_characteristics.truck(kind);
        let den = TYPES.iter().try_fold(1i64, |d, t| {
            d.checked_mul(i64::from(chart.supply_capacity(*t)))
        })?;
        if den <= 0 {
            return None;
        }
        rooms[i] = i64::from(n).checked_mul(den)?;
        for (t, ty) in TYPES.iter().enumerate() {
            cost[i][t] = den / i64::from(chart.supply_capacity(*ty));
        }
    }
    let demand = [stock.ammo, stock.stores, stock.fuel, stock.water];
    if demand.iter().any(|n| *n < 0) {
        return None;
    }
    let mut search = Search {
        demand,
        cost,
        allocation: [[0; 4]; 6],
        failed: BTreeSet::new(),
    };
    if !search.place(0, 0, demand[0], rooms) {
        return None;
    }
    let mut out = [CargoPacking::default(), CargoPacking::default()];
    for i in 0..6 {
        let ns = search.allocation[i];
        let cargo = Supplies {
            ammo: ns[0],
            stores: ns[1],
            fuel: ns[2],
            water: ns[3],
        };
        match KINDS[i % 3] {
            TruckType::Heavy => out[i / 3].heavy = cargo,
            TruckType::Medium => out[i / 3].medium = cargo,
            TruckType::Light => out[i / 3].light = cargo,
        }
    }
    let [a, b] = out;
    Some((a, b))
}
