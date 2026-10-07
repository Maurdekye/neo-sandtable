//! Controlled integer rounding keeps both unit and equipment proportions.
use super::{Asset, Equipment, valid_allocation};
use cna_core::ids::UnitId;
use std::collections::{BTreeMap, VecDeque};

/// A physical cell's carrier identity is independent of the Land unit map.
/// Cases: land:21.36
#[derive(Debug, Clone)]
pub struct CarrierAsset {
    pub carrier: String,
    pub equipment: Equipment,
    pub points: i32,
}
fn neutral(assets: &[Asset]) -> Vec<CarrierAsset> {
    assets
        .iter()
        .map(|a| CarrierAsset {
            carrier: a.unit.to_string(),
            equipment: a.equipment.clone(),
            points: a.points,
        })
        .collect()
}
/// Existing unit API, unchanged; its carrier-neutral implementation also accepts real pool ids.
/// Cases: land:21.36
pub fn valid_group_allocation(assets: &[Asset], broken: i32, allocation: &[i32]) -> bool {
    valid_carrier_allocation(&neutral(assets), broken, allocation)
}
/// Existing unit baseline API, unchanged.
/// Cases: land:21.36
pub fn balanced_allocation(assets: &[Asset], broken: i32) -> Option<Vec<i32>> {
    balanced_carrier_allocation(&neutral(assets), broken)
}
pub(super) fn grouped(assets: &[Asset]) -> Option<(Vec<Asset>, Vec<Vec<usize>>)> {
    let (cells, indices) = grouped_carrier(&neutral(assets))?;
    Some((
        cells
            .into_iter()
            .map(|a| Asset {
                unit: UnitId::new(a.carrier),
                equipment: a.equipment,
                points: a.points,
                cohort: None,
            })
            .collect(),
        indices,
    ))
}
pub(super) fn expand(assets: &[Asset], indices: &[Vec<usize>], chosen: &[i32]) -> Option<Vec<i32>> {
    expand_carrier(&neutral(assets), indices, chosen)
}
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
/// Cohort identifiers distinguish physical history, not another vehicle type.
/// Cases: land:21.36
pub(super) fn grouped_carrier(
    assets: &[CarrierAsset],
) -> Option<(Vec<CarrierAsset>, Vec<Vec<usize>>)> {
    let mut cells: BTreeMap<(String, Equipment), (i32, Vec<usize>)> = BTreeMap::new();
    for (i, a) in assets.iter().enumerate() {
        if a.points < 0 {
            return None;
        }
        let e = cells
            .entry((a.carrier.clone(), a.equipment.clone()))
            .or_default();
        e.0 = e.0.checked_add(a.points)?;
        e.1.push(i);
    }
    let mut out = vec![];
    let mut indices = vec![];
    for ((carrier, equipment), (points, ii)) in cells {
        out.push(CarrierAsset {
            carrier,
            equipment,
            points,
        });
        indices.push(ii);
    }
    Some((out, indices))
}
pub(super) fn expand_carrier(
    assets: &[CarrierAsset],
    indices: &[Vec<usize>],
    chosen: &[i32],
) -> Option<Vec<i32>> {
    let mut out = vec![0; assets.len()];
    for (ii, n) in indices.iter().zip(chosen) {
        let mut left = *n;
        for i in ii {
            out[*i] = left.min(assets[*i].points);
            left -= out[*i];
        }
        if left != 0 {
            return None;
        }
    }
    Some(out)
}
/// Both unit totals and vehicle-type totals lie in their proportional floor/ceiling quotas.
/// Different owner tie choices remain legal; all rolled losses must be assigned.
/// Cases: land:21.36
pub fn valid_carrier_allocation(assets: &[CarrierAsset], broken: i32, allocation: &[i32]) -> bool {
    if allocation.len() != assets.len()
        || assets
            .iter()
            .zip(allocation)
            .any(|(a, n)| *n < 0 || *n > a.points)
    {
        return false;
    }
    let Some((cells, indices)) = grouped_carrier(assets) else {
        return false;
    };
    let Some(chosen) = indices
        .iter()
        .map(|ii| {
            ii.iter()
                .try_fold(0i32, |n, i| n.checked_add(allocation[*i]))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    valid_cells(&cells, broken, &chosen)
}
fn valid_cells(assets: &[CarrierAsset], broken: i32, allocation: &[i32]) -> bool {
    let points: Vec<_> = assets.iter().map(|a| a.points).collect();
    if !valid_allocation(&points, broken, allocation) {
        return false;
    }
    let mut units: BTreeMap<String, (i32, i32)> = BTreeMap::new();
    let mut kinds: BTreeMap<Equipment, (i32, i32)> = BTreeMap::new();
    for (a, n) in assets.iter().zip(allocation) {
        for totals in [
            units.entry(a.carrier.clone()).or_default(),
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
pub fn balanced_carrier_allocation(assets: &[CarrierAsset], broken: i32) -> Option<Vec<i32>> {
    let (cells, indices) = grouped_carrier(assets)?;
    let chosen = balanced_cells(&cells, broken)?;
    expand_carrier(assets, &indices, &chosen)
}
fn balanced_cells(assets: &[CarrierAsset], broken: i32) -> Option<Vec<i32>> {
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
    let units: BTreeMap<_, _> = assets.iter().map(|a| (a.carrier.clone(), ())).collect();
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
        let r = units[&a.carrier];
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
    valid_carrier_allocation(assets, broken, &result).then_some(result)
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
                                cohort: None,
                            },
                            Asset {
                                unit: "a".into(),
                                equipment: Equipment::HeavyTruck,
                                points: b,
                                cohort: None,
                            },
                            Asset {
                                unit: "b".into(),
                                equipment: Equipment::LightTruck,
                                points: c,
                                cohort: None,
                            },
                            Asset {
                                unit: "b".into(),
                                equipment: Equipment::HeavyTruck,
                                points: d,
                                cohort: None,
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
                cohort: None,
            })
            .collect();
        assert!(valid_allocation(&[1; 6], 3, &[1, 1, 1, 0, 0, 0]));
        assert!(!valid_group_allocation(&assets, 3, &[1, 1, 1, 0, 0, 0]));
        let out = balanced_allocation(&assets, 3).unwrap();
        assert!(valid_group_allocation(&assets, 3, &out));
    }
    /// Cases: land:21.36
    #[test]
    fn identical_vehicle_cohorts_have_one_proportion_and_owner_selects_exact_history() {
        let a = Asset {
            unit: "a".into(),
            equipment: Equipment::LightTruck,
            points: 2,
            cohort: Some("old".into()),
        };
        let b = Asset {
            points: 100,
            cohort: Some("new".into()),
            ..a.clone()
        };
        assert!(valid_group_allocation(
            &[a.clone(), b.clone()],
            50,
            &[2, 48]
        ));
        assert!(valid_group_allocation(
            &[a.clone(), b.clone()],
            50,
            &[0, 50]
        ));
        assert!(!valid_group_allocation(
            &[a.clone(), b.clone()],
            50,
            &[3, 47]
        ));
        assert_eq!(balanced_allocation(&[a, b], 50), Some(vec![2, 48]));
    }
}
