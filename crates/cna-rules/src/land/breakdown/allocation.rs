//! Controlled integer rounding keeps both unit and equipment proportions.
use super::{Asset, Equipment, valid_allocation};
use cna_core::ids::UnitId;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone)]
struct Edge {
    to: usize,
    reverse: usize,
    capacity: i64,
    initial: i64,
}
fn edge(g: &mut [Vec<Edge>], a: usize, b: usize, n: i64) -> usize {
    let i = g[a].len();
    let j = g[b].len();
    g[a].push(Edge {
        to: b,
        reverse: j,
        capacity: n,
        initial: n,
    });
    g[b].push(Edge {
        to: a,
        reverse: i,
        capacity: 0,
        initial: 0,
    });
    i
}
fn bounded(
    g: &mut [Vec<Edge>],
    d: &mut [i64],
    a: usize,
    b: usize,
    lo: i64,
    hi: i64,
) -> Option<usize> {
    if lo < 0 || hi < lo {
        return None;
    }
    d[a] -= lo;
    d[b] += lo;
    Some(edge(g, a, b, hi - lo))
}
fn maxflow(g: &mut [Vec<Edge>], start: usize, end: usize) -> i64 {
    let mut flow = 0;
    loop {
        let mut prev = vec![None; g.len()];
        let mut q = VecDeque::from([start]);
        prev[start] = Some((start, 0));
        while let Some(v) = q.pop_front() {
            for (i, e) in g[v].iter().enumerate() {
                if e.capacity > 0 && prev[e.to].is_none() {
                    prev[e.to] = Some((v, i));
                    q.push_back(e.to);
                }
            }
        }
        if prev[end].is_none() {
            return flow;
        }
        let mut v = end;
        let mut amount = i64::MAX;
        while v != start {
            let (a, i) = prev[v].unwrap();
            amount = amount.min(g[a][i].capacity);
            v = a;
        }
        v = end;
        while v != start {
            let (a, i) = prev[v].unwrap();
            let r = g[a][i].reverse;
            g[a][i].capacity -= amount;
            g[v][r].capacity += amount;
            v = a;
        }
        flow += amount;
    }
}
/// Both unit totals and vehicle-type totals lie in their proportional floor/ceiling quotas.
/// Different owner tie choices remain legal; all rolled losses must be assigned.
/// Cases: land:21.36
pub fn valid_group_allocation(assets: &[Asset], broken: i32, allocation: &[i32]) -> bool {
    let points: Vec<_> = assets.iter().map(|a| a.points).collect();
    if !valid_allocation(&points, broken, allocation) {
        return false;
    }
    let mut units: BTreeMap<UnitId, (i32, i32)> = BTreeMap::new();
    let mut kinds: BTreeMap<Equipment, (i32, i32)> = BTreeMap::new();
    for (a, n) in assets.iter().zip(allocation) {
        for totals in [
            units.entry(a.unit.clone()).or_default(),
            kinds.entry(a.equipment.clone()).or_default(),
        ] {
            let Some(p) = totals.0.checked_add(a.points) else {
                return false;
            };
            let Some(l) = totals.1.checked_add(*n) else {
                return false;
            };
            *totals = (p, l);
        }
    }
    for group in [
        units.values().copied().collect::<Vec<_>>(),
        kinds.values().copied().collect(),
    ] {
        let (p, l): (Vec<_>, Vec<_>) = group.into_iter().unzip();
        if !valid_allocation(&p, broken, &l) {
            return false;
        }
    }
    true
}
/// A bounded integral flow chooses a legal joint rounding without enumerating every tie.
/// This is controller assistance: the owning player can submit another valid allocation.
/// Cases: land:21.36
pub fn balanced_allocation(assets: &[Asset], broken: i32) -> Option<Vec<i32>> {
    if broken < 0 || assets.iter().any(|a| a.points < 0) {
        return None;
    }
    let total: i64 = assets.iter().map(|a| i64::from(a.points)).sum();
    if i64::from(broken) > total {
        return None;
    }
    if total == 0 {
        return Some(vec![0; assets.len()]);
    }
    let units: BTreeMap<_, _> = assets.iter().map(|a| (a.unit.clone(), ())).collect();
    let kinds: BTreeMap<_, _> = assets.iter().map(|a| (a.equipment.clone(), ())).collect();
    let units: BTreeMap<_, _> = units.into_keys().enumerate().map(|(i, k)| (k, i)).collect();
    let kinds: BTreeMap<_, _> = kinds.into_keys().enumerate().map(|(i, k)| (k, i)).collect();
    let rows = units.len();
    let cols = kinds.len();
    let source = rows + cols;
    let sink = source + 1;
    let ss = sink + 1;
    let tt = ss + 1;
    let mut g = vec![vec![]; tt + 1];
    let mut demands = vec![0; tt + 1];
    let mut result: Vec<i32> = assets
        .iter()
        .map(|a| (i64::from(a.points) * i64::from(broken) / total) as i32)
        .collect();
    let mut row_points = vec![0i64; rows];
    let mut col_points = vec![0i64; cols];
    let mut row_floor = vec![0i64; rows];
    let mut col_floor = vec![0i64; cols];
    let mut edges = vec![None; assets.len()];
    for (i, a) in assets.iter().enumerate() {
        let r = units[&a.unit];
        let c = kinds[&a.equipment];
        row_points[r] += i64::from(a.points);
        col_points[c] += i64::from(a.points);
        row_floor[r] += i64::from(result[i]);
        col_floor[c] += i64::from(result[i]);
        if i64::from(a.points) * i64::from(broken) % total > 0 {
            edges[i] = Some((r, edge(&mut g, r, rows + c, 1)));
        }
    }
    let quota = |points: i64, base: i64| {
        let n = points * i64::from(broken);
        (n / total - base, (n + total - 1) / total - base)
    };
    for r in 0..rows {
        let (lo, hi) = quota(row_points[r], row_floor[r]);
        bounded(&mut g, &mut demands, source, r, lo, hi)?;
    }
    for c in 0..cols {
        let (lo, hi) = quota(col_points[c], col_floor[c]);
        bounded(&mut g, &mut demands, rows + c, sink, lo, hi)?;
    }
    let remainder = i64::from(broken) - result.iter().map(|n| i64::from(*n)).sum::<i64>();
    bounded(&mut g, &mut demands, sink, source, remainder, remainder)?;
    let mut required = 0;
    for (v, d) in demands.iter().copied().enumerate().take(ss) {
        if d > 0 {
            edge(&mut g, ss, v, d);
            required += d;
        } else if d < 0 {
            edge(&mut g, v, tt, -d);
        }
    }
    if maxflow(&mut g, ss, tt) != required {
        return None;
    }
    for (i, e) in edges.into_iter().enumerate() {
        if let Some((r, j)) = e {
            result[i] += (g[r][j].initial - g[r][j].capacity) as i32;
        }
    }
    valid_group_allocation(assets, broken, &result).then_some(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Cases: land:21.36
    #[test]
    fn every_two_unit_two_type_rounding_conserves_both_margins() {
        for a in 0..=4 {
            for b in 0..=4 {
                for c in 0..=4 {
                    for d in 0..=4 {
                        let assets = vec![
                            Asset {
                                unit: "a".into(),
                                equipment: Equipment::LightTruck,
                                points: a,
                            },
                            Asset {
                                unit: "a".into(),
                                equipment: Equipment::HeavyTruck,
                                points: b,
                            },
                            Asset {
                                unit: "b".into(),
                                equipment: Equipment::LightTruck,
                                points: c,
                            },
                            Asset {
                                unit: "b".into(),
                                equipment: Equipment::HeavyTruck,
                                points: d,
                            },
                        ];
                        for n in 0..=a + b + c + d {
                            let out = balanced_allocation(&assets, n).unwrap();
                            assert!(valid_group_allocation(&assets, n, &out));
                        }
                    }
                }
            }
        }
    }
    /// Cases: land:21.36
    #[test]
    fn individual_cell_quotas_do_not_authorize_an_imbalanced_unit_total() {
        let assets: Vec<_> = (0..6)
            .map(|i| Asset {
                unit: if i < 3 { "a" } else { "b" }.into(),
                equipment: Equipment::Weapon(format!("weapon{}", i % 3)),
                points: 1,
            })
            .collect();
        assert!(valid_allocation(&[1; 6], 3, &[1, 1, 1, 0, 0, 0]));
        assert!(!valid_group_allocation(&assets, 3, &[1, 1, 1, 0, 0, 0]));
        let out = balanced_allocation(&assets, 3).unwrap();
        assert!(valid_group_allocation(&assets, 3, &out));
    }
}
