//! Explicit truck and cargo allocations for barrage losses.
use super::{CargoPacking, SUPPLIES, TYPES, points, set_points, trucks, validate_packing};
use crate::{
    CnaContent,
    logistics::{SupplyError, toe_strength},
    state::State,
};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_core::ids::UnitId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// All cargo assignments are explicit, including empty and personnel-carrying trucks.
/// This helper does not apply infantry TOE casualties; call it before that mutation
/// in the same transactional combat draft.
/// Cases: land:12.46, airlog:54.2
/// Interpretations: interp:land-0024
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitTruckCargoLoss {
    pub unit: UnitId,
    pub infantry_motorized_toe_lost: i32,
    pub infantry_carriers_lost: Trucks,
    pub chart_losses: Trucks,
    pub chart_transport_losses: Trucks,
    pub before: CargoPacking,
    pub lost: CargoPacking,
    pub surviving: CargoPacking,
}

/// The actual chart count is capped only after additional infantry carriers disappear.
/// Cases: land:12.46
/// Interpretations: interp:land-0024
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruckCargoLossReport {
    pub infantry_carriers_lost: Trucks,
    pub chart_losses: Trucks,
    pub chart_transport_losses: Trucks,
    pub cargo_lost: Supplies,
}

fn checked_sum(t: &Trucks) -> Result<i64, SupplyError> {
    TYPES.into_iter().try_fold(0i64, |n, k| {
        let v = trucks(t, k);
        if v < 0 {
            return Err(SupplyError::Invalid);
        }
        n.checked_add(i64::from(v)).ok_or(SupplyError::Invalid)
    })
}
fn set_trucks(t: &mut Trucks, k: cna_tables::airlog::trucks::TruckType, n: i32) {
    use cna_tables::airlog::trucks::TruckType::*;
    match k {
        Light => t.light = n,
        Medium => t.medium = n,
        Heavy => t.heavy = n,
    }
}
fn subtract(a: &Trucks, b: &Trucks) -> Result<Trucks, SupplyError> {
    checked_sum(a)?;
    checked_sum(b)?;
    let mut out = Trucks::default();
    for k in TYPES {
        let n = trucks(a, k)
            .checked_sub(trucks(b, k))
            .ok_or(SupplyError::Invalid)?;
        if n < 0 {
            return Err(SupplyError::Invalid);
        }
        set_trucks(&mut out, k, n);
    }
    Ok(out)
}
fn add_trucks(a: &mut Trucks, b: &Trucks) -> Result<(), SupplyError> {
    checked_sum(b)?;
    for k in TYPES {
        let n = trucks(a, k)
            .checked_add(trucks(b, k))
            .ok_or(SupplyError::Invalid)?;
        set_trucks(a, k, n);
    }
    Ok(())
}
fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}
fn denominator(content: &CnaContent) -> Result<i128, SupplyError> {
    let mut den = 1i128;
    for k in TYPES {
        for s in SUPPLIES {
            let n = i128::from(
                content
                    .tables
                    .airlog
                    .truck_characteristics
                    .truck(k)
                    .supply_capacity(s),
            );
            if n <= 0 {
                return Err(SupplyError::Unsupported {
                    case: "airlog:54.2",
                });
            }
            den = (den / gcd(den, n))
                .checked_mul(n)
                .ok_or(SupplyError::Invalid)?;
        }
    }
    Ok(den)
}
// Ammo, fuel, stores, water, personnel, empty, all in exact common-denominator units.
fn shares(
    content: &CnaContent,
    attached: &Trucks,
    transport: &Trucks,
    packing: &CargoPacking,
    den: i128,
) -> Result<[i128; 6], SupplyError> {
    let mut out = [0i128; 6];
    for k in TYPES {
        for (index, s) in SUPPLIES.into_iter().enumerate() {
            let cap = i128::from(
                content
                    .tables
                    .airlog
                    .truck_characteristics
                    .truck(k)
                    .supply_capacity(s),
            );
            out[index] += i128::from(points(packing.cargo(k), s)) * (den / cap);
        }
    }
    out[4] = i128::from(checked_sum(transport)?) * den;
    let used: i128 = out[..5].iter().sum();
    out[5] = i128::from(checked_sum(attached)?) * den - used;
    if out.iter().any(|n| *n < 0) {
        return Err(SupplyError::Invalid);
    }
    Ok(out)
}
fn balanced<const N: usize>(
    available: &[i128; N],
    lost: &[i128; N],
    one: i128,
) -> Result<(), SupplyError> {
    for i in 0..N {
        if lost[i] < 0 || lost[i] > available[i] {
            return Err(SupplyError::Invalid);
        }
        for j in 0..N {
            if lost[i] > lost[j] + one && lost[j] < available[j] {
                return Err(SupplyError::Invalid);
            }
        }
    }
    Ok(())
}
fn minimum_carriers(
    content: &CnaContent,
    assigned: &Trucks,
    lost_toe: i32,
) -> Result<i64, SupplyError> {
    if lost_toe < 0 {
        return Err(SupplyError::Invalid);
    }
    let mut needed = i64::from(lost_toe) * 2;
    let mut caps: Vec<_> = TYPES
        .into_iter()
        .map(|k| {
            (
                content
                    .tables
                    .airlog
                    .truck_characteristics
                    .truck(k)
                    .capacity_inf_toe_halves,
                trucks(assigned, k),
            )
        })
        .collect();
    caps.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut count = 0i64;
    for (cap, have) in caps {
        if cap <= 0 || have < 0 {
            return Err(SupplyError::Invalid);
        }
        if needed <= 0 {
            break;
        }
        let n = i64::from(have).min((needed + i64::from(cap) - 1) / i64::from(cap));
        count += n;
        needed -= n * i64::from(cap);
    }
    if needed > 0 {
        return Err(SupplyError::Insufficient);
    }
    Ok(count)
}

/// Validate one defender allocation across the entire caller-specified eligible set.
/// All eligible units must be friendly, active and co-located; an allocation is required
/// even for an eligible unit taking no loss. The caller supplies that set from the target
/// and parent relationship. Additional infantry carriers are removed first; the chart
/// count is then capped and balanced across truck types and exact cargo capacity shares.
/// The three packings conserve each supply and truck type without an inferred loss rate.
/// Chart-hit troop carriers reduce transport allocation, without additional TOE deaths.
/// Quantities and assignments in the returned report are owner-private information.
/// Cases: land:12.46, airlog:53.11, airlog:54.2
/// Interpretations: interp:land-0024, interp:airlog-0008
pub fn apply_truck_cargo_loss(
    content: &CnaContent,
    state: &mut State,
    eligible: &[UnitId],
    required_chart_loss: i32,
    allocations: &[UnitTruckCargoLoss],
) -> Result<TruckCargoLossReport, SupplyError> {
    if required_chart_loss < 0 {
        return Err(SupplyError::Invalid);
    }
    if eligible.is_empty() {
        return if allocations.is_empty() {
            Ok(TruckCargoLossReport::default())
        } else {
            Err(SupplyError::Invalid)
        };
    }
    let domain: BTreeSet<_> = eligible.iter().cloned().collect();
    let chosen: BTreeSet<_> = allocations.iter().map(|a| a.unit.clone()).collect();
    if domain.len() != eligible.len() || chosen.len() != allocations.len() || chosen != domain {
        return Err(SupplyError::Invalid);
    }
    let first = state
        .land
        .units
        .get(&eligible[0])
        .ok_or(SupplyError::Invalid)?;
    if !matches!(first.location, crate::state::Location::Hex { .. }) {
        return Err(SupplyError::Invalid);
    }
    let side = first.side;
    let location = first.location.clone();
    let den = denominator(content)?;
    let mut available_types = [0i128; 3];
    let mut lost_types = [0i128; 3];
    let mut available_cargo = [0i128; 6];
    let mut lost_cargo = [0i128; 6];
    let mut report = TruckCargoLossReport::default();
    let mut changes = Vec::new();
    for a in allocations {
        let unit = state.land.units.get(&a.unit).ok_or(SupplyError::Invalid)?;
        if unit.side != side || unit.location != location {
            return Err(SupplyError::Invalid);
        }
        let stock = state
            .logistics
            .unit_supply
            .get(&a.unit)
            .cloned()
            .unwrap_or_default();
        validate_packing(
            content,
            &unit.trucks,
            &unit.transport_trucks,
            &stock.carried,
            &a.before,
        )?;
        let remaining = subtract(&unit.trucks, &a.infantry_carriers_lost)?;
        let remaining_transport = subtract(&unit.transport_trucks, &a.infantry_carriers_lost)?;
        let minimum = minimum_carriers(
            content,
            &unit.transport_trucks,
            a.infantry_motorized_toe_lost,
        )?;
        if checked_sum(&a.infantry_carriers_lost)? != minimum {
            return Err(SupplyError::Invalid);
        }
        if a.infantry_motorized_toe_lost > 0 {
            let class = crate::logistics::rations::class(content, &a.unit)?;
            if !matches!(class.unit_type.as_str(), "infantry" | "engineer")
                || a.infantry_motorized_toe_lost > toe_strength(content, unit)?.get()
            {
                return Err(SupplyError::Invalid);
            }
            let covering: i64 = TYPES
                .into_iter()
                .map(|k| {
                    i64::from(trucks(&a.infantry_carriers_lost, k))
                        * i64::from(
                            content
                                .tables
                                .airlog
                                .truck_characteristics
                                .truck(k)
                                .capacity_inf_toe_halves,
                        )
                })
                .sum();
            if covering < i64::from(a.infantry_motorized_toe_lost) * 2 {
                return Err(SupplyError::Insufficient);
            }
        }
        let surviving = subtract(&remaining, &a.chart_losses)?;
        let surviving_transport = subtract(&remaining_transport, &a.chart_transport_losses)?;
        subtract(&a.chart_losses, &a.chart_transport_losses)?;
        let lost = a.lost.totals()?;
        let kept = a.surviving.totals()?;
        for k in TYPES {
            for s in SUPPLIES {
                let n = points(a.lost.cargo(k), s)
                    .checked_add(points(a.surviving.cargo(k), s))
                    .ok_or(SupplyError::Invalid)?;
                if n != points(a.before.cargo(k), s) {
                    return Err(SupplyError::Invalid);
                }
            }
        }
        validate_packing(
            content,
            &remaining,
            &remaining_transport,
            &stock.carried,
            &a.before,
        )?;
        validate_packing(
            content,
            &a.chart_losses,
            &a.chart_transport_losses,
            &lost,
            &a.lost,
        )?;
        validate_packing(
            content,
            &surviving,
            &surviving_transport,
            &kept,
            &a.surviving,
        )?;
        for (i, k) in TYPES.into_iter().enumerate() {
            available_types[i] += i128::from(trucks(&remaining, k));
            lost_types[i] += i128::from(trucks(&a.chart_losses, k));
        }
        let available = shares(content, &remaining, &remaining_transport, &a.before, den)?;
        let destroyed = shares(
            content,
            &a.chart_losses,
            &a.chart_transport_losses,
            &a.lost,
            den,
        )?;
        for i in 0..6 {
            available_cargo[i] += available[i];
            lost_cargo[i] += destroyed[i];
        }
        add_trucks(
            &mut report.infantry_carriers_lost,
            &a.infantry_carriers_lost,
        )?;
        add_trucks(&mut report.chart_losses, &a.chart_losses)?;
        add_trucks(
            &mut report.chart_transport_losses,
            &a.chart_transport_losses,
        )?;
        for s in SUPPLIES {
            let n = points(&report.cargo_lost, s)
                .checked_add(points(&lost, s))
                .ok_or(SupplyError::Invalid)?;
            set_points(&mut report.cargo_lost, s, n);
        }
        changes.push((a.unit.clone(), surviving, surviving_transport, kept));
    }
    let available: i128 = available_types.iter().sum();
    if lost_types.iter().sum::<i128>() != i128::from(required_chart_loss).min(available) {
        return Err(SupplyError::Invalid);
    }
    balanced(&available_types, &lost_types, 1)?;
    balanced(&available_cargo, &lost_cargo, den)?;
    for (id, attached, transport, carried) in changes {
        let unit = state.land.units.get_mut(&id).expect("validated unit");
        unit.trucks = attached;
        unit.transport_trucks = transport;
        if let Some(stock) = state.logistics.unit_supply.get_mut(&id) {
            stock.carried = carried;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Location, UnitSupply};

    fn fixture() -> (CnaContent, State, Vec<UnitId>) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let ids: Vec<UnitId> = [
            "it.1_libyan_div.viii_libyan_bn",
            "it.1_libyan_div.i_libyan_bn",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        // Select a second real infantry identity rather than relying on a chart class.
        let first = ids[0].clone();
        let second = s
            .land
            .units
            .values()
            .find(|u| {
                u.id != first
                    && u.side == s.land.units[&first].side
                    && crate::logistics::rations::class(&c, &u.id)
                        .is_ok_and(|x| x.unit_type == "infantry")
            })
            .unwrap()
            .id
            .clone();
        let ids = vec![first, second];
        for id in &ids {
            let u = s.land.units.get_mut(id).unwrap();
            u.location = Location::Hex {
                hex: "C4022".into(),
            };
            u.trucks = Trucks::default();
            u.transport_trucks = Trucks::default();
            s.logistics
                .unit_supply
                .insert(id.clone(), UnitSupply::default());
        }
        (c, s, ids)
    }
    fn allocation(id: &UnitId) -> UnitTruckCargoLoss {
        UnitTruckCargoLoss {
            unit: id.clone(),
            infantry_motorized_toe_lost: 0,
            infantry_carriers_lost: Trucks::default(),
            chart_losses: Trucks::default(),
            chart_transport_losses: Trucks::default(),
            before: CargoPacking::default(),
            lost: CargoPacking::default(),
            surviving: CargoPacking::default(),
        }
    }
    fn snapshot(s: &State) -> serde_json::Value {
        serde_json::to_value(s).unwrap()
    }

    /// Cases: land:12.46, airlog:54.2
    /// Interpretations: interp:land-0024
    #[test]
    fn infantry_carriers_die_first_then_chart_is_capped_and_cargo_is_conserved() {
        let (c, mut s, ids) = fixture();
        let id = &ids[0];
        let u = s.land.units.get_mut(id).unwrap();
        u.trucks = Trucks {
            light: 2,
            medium: 2,
            heavy: 2,
        };
        u.transport_trucks.heavy = 1;
        let mut a = allocation(id);
        a.infantry_motorized_toe_lost = 1;
        a.infantry_carriers_lost.heavy = 1;
        a.chart_losses = Trucks {
            light: 1,
            medium: 1,
            heavy: 1,
        };
        a.before.heavy.stores = 30;
        a.lost = a.before.clone();
        s.logistics.unit_supply.get_mut(id).unwrap().carried = a.before.totals().unwrap();
        let original_toe = s.land.units[id].toe.clone();
        let report = apply_truck_cargo_loss(&c, &mut s, std::slice::from_ref(id), 3, &[a]).unwrap();
        assert_eq!(report.infantry_carriers_lost.heavy, 1);
        assert_eq!(
            report.chart_losses,
            Trucks {
                light: 1,
                medium: 1,
                heavy: 1
            }
        );
        assert_eq!(report.cargo_lost.stores, 30);
        assert_eq!(
            s.land.units[id].trucks,
            Trucks {
                light: 1,
                medium: 1,
                heavy: 0
            }
        );
        assert_eq!(s.land.units[id].toe, original_toe);
        assert_eq!(s.logistics.unit_supply[id].carried, Supplies::default());

        // The chart may request more than remains after the separate carrier loss.
        let (c, mut s, ids) = fixture();
        let id = &ids[0];
        s.land.units.get_mut(id).unwrap().trucks.heavy = 2;
        s.land.units.get_mut(id).unwrap().transport_trucks.heavy = 2;
        let mut a = allocation(id);
        a.infantry_motorized_toe_lost = 1;
        a.infantry_carriers_lost.heavy = 1;
        a.chart_losses.heavy = 1;
        a.chart_transport_losses.heavy = 1;
        let report =
            apply_truck_cargo_loss(&c, &mut s, std::slice::from_ref(id), 99, &[a]).unwrap();
        assert_eq!(report.chart_losses.heavy, 1);
        assert_eq!(s.land.units[id].trucks.heavy, 0);
        assert_eq!(s.land.units[id].transport_trucks.heavy, 0);
    }

    /// Cases: land:12.46
    /// Interpretations: interp:land-0024
    #[test]
    fn chart_hits_may_dismount_survivors_without_killing_more_infantry() {
        let (c, mut s, ids) = fixture();
        let id = &ids[0];
        let u = s.land.units.get_mut(id).unwrap();
        u.trucks.heavy = 2;
        u.transport_trucks.heavy = 1;
        let toe = u.toe.clone();
        let mut a = allocation(id);
        a.before.heavy.stores = 30;
        a.surviving = a.before.clone();
        a.chart_losses.heavy = 1;
        a.chart_transport_losses.heavy = 1;
        s.logistics.unit_supply.get_mut(id).unwrap().carried = a.before.totals().unwrap();
        let report = apply_truck_cargo_loss(&c, &mut s, std::slice::from_ref(id), 1, &[a]).unwrap();
        assert_eq!(report.chart_transport_losses.heavy, 1);
        assert_eq!(s.land.units[id].transport_trucks.heavy, 0);
        assert_eq!(s.land.units[id].toe, toe);
        assert_eq!(s.logistics.unit_supply[id].carried.stores, 30);
    }

    /// Cases: land:12.46, airlog:54.2
    /// Interpretations: interp:land-0024
    #[test]
    fn cargo_balance_uses_exact_truck_shares_and_empty_is_a_category() {
        let (c, mut s, ids) = fixture();
        let id = &ids[0];
        s.land.units.get_mut(id).unwrap().trucks.heavy = 4;
        let mut a = allocation(id);
        a.before.heavy.ammo = 16;
        a.before.heavy.fuel = 250;
        a.chart_losses.heavy = 2;
        a.lost.heavy.ammo = 8; // one truck-equivalent
        a.lost.heavy.fuel = 125; // 0.5 truck-equivalents
        a.surviving.heavy.ammo = 8;
        a.surviving.heavy.fuel = 125;
        s.logistics.unit_supply.get_mut(id).unwrap().carried = a.before.totals().unwrap();
        let before = snapshot(&s);
        let mut bad = a.clone();
        bad.lost.heavy.ammo = 13; // difference > one even though rounded points seem close
        bad.lost.heavy.fuel = 0;
        bad.surviving.heavy.ammo = 3;
        bad.surviving.heavy.fuel = 250;
        // Both individual packings fit: the rejection is the exact cargo imbalance.
        validate_packing(
            &c,
            &bad.chart_losses,
            &Trucks::default(),
            &bad.lost.totals().unwrap(),
            &bad.lost,
        )
        .unwrap();
        validate_packing(
            &c,
            &bad.chart_losses,
            &Trucks::default(),
            &bad.surviving.totals().unwrap(),
            &bad.surviving,
        )
        .unwrap();
        assert_eq!(
            apply_truck_cargo_loss(&c, &mut s, std::slice::from_ref(id), 2, &[bad]),
            Err(SupplyError::Invalid)
        );
        assert_eq!(snapshot(&s), before);
        apply_truck_cargo_loss(&c, &mut s, std::slice::from_ref(id), 2, &[a]).unwrap();

        let (c, mut s, ids) = fixture();
        let id = &ids[0];
        s.land.units.get_mut(id).unwrap().trucks = Trucks {
            light: 0,
            medium: 2,
            heavy: 2,
        };
        let mut a = allocation(id);
        a.before.medium.stores = 15;
        a.before.heavy.stores = 30;
        a.chart_losses = Trucks {
            light: 0,
            medium: 1,
            heavy: 1,
        };
        a.lost = a.before.clone(); // two store trucks, no empty losses: unbalanced
        s.logistics.unit_supply.get_mut(id).unwrap().carried = a.before.totals().unwrap();
        let before = snapshot(&s);
        assert_eq!(
            apply_truck_cargo_loss(
                &c,
                &mut s,
                std::slice::from_ref(id),
                2,
                std::slice::from_ref(&a)
            ),
            Err(SupplyError::Invalid)
        );
        assert_eq!(snapshot(&s), before);
        a.lost.heavy.stores = 0;
        a.surviving.heavy.stores = 30; // now one store and one empty truck
        apply_truck_cargo_loss(&c, &mut s, std::slice::from_ref(id), 2, &[a]).unwrap();
    }

    /// Cases: land:12.46
    /// Interpretations: interp:land-0024
    #[test]
    fn evenness_is_across_eligible_units_and_invalid_last_allocation_is_atomic() {
        let (c, mut s, ids) = fixture();
        s.land.units.get_mut(&ids[0]).unwrap().trucks.light = 2;
        s.land.units.get_mut(&ids[1]).unwrap().trucks.medium = 2;
        let mut a = allocation(&ids[0]);
        let mut b = allocation(&ids[1]);
        a.chart_losses.light = 1;
        b.chart_losses.medium = 1;
        let before = snapshot(&s);
        let mut bad = b.clone();
        bad.chart_losses.medium = 3;
        assert!(apply_truck_cargo_loss(&c, &mut s, &ids, 2, &[a.clone(), bad]).is_err());
        assert_eq!(snapshot(&s), before);
        apply_truck_cargo_loss(&c, &mut s, &ids, 2, &[a, b]).unwrap();
        assert_eq!(s.land.units[&ids[0]].trucks.light, 1);
        assert_eq!(s.land.units[&ids[1]].trucks.medium, 1);
        let restored: State = serde_json::from_value(snapshot(&s)).unwrap();
        assert_eq!(snapshot(&s), snapshot(&restored));
    }

    /// Cases: land:12.46, airlog:54.2
    /// Interpretations: interp:land-0024
    #[test]
    fn insufficient_or_excess_carriers_missing_units_and_bad_packing_reject() {
        let (c, mut s, ids) = fixture();
        let id = &ids[0];
        s.land.units.get_mut(id).unwrap().trucks = Trucks {
            light: 2,
            medium: 0,
            heavy: 1,
        };
        s.land.units.get_mut(id).unwrap().transport_trucks = Trucks {
            light: 2,
            medium: 0,
            heavy: 1,
        };
        let mut a = allocation(id);
        a.infantry_motorized_toe_lost = 1;
        a.infantry_carriers_lost.light = 2; // one heavy is the minimum instead
        let before = snapshot(&s);
        assert_eq!(
            apply_truck_cargo_loss(
                &c,
                &mut s,
                std::slice::from_ref(id),
                0,
                std::slice::from_ref(&a)
            ),
            Err(SupplyError::Invalid)
        );
        a.infantry_carriers_lost = Trucks {
            light: 1,
            medium: 0,
            heavy: 0,
        };
        assert!(
            apply_truck_cargo_loss(
                &c,
                &mut s,
                std::slice::from_ref(id),
                0,
                std::slice::from_ref(&a)
            )
            .is_err()
        );
        a.infantry_carriers_lost = Trucks {
            light: 0,
            medium: 0,
            heavy: 1,
        };
        a.lost.light.fuel = 1;
        assert!(apply_truck_cargo_loss(&c, &mut s, std::slice::from_ref(id), 0, &[a]).is_err());
        assert_eq!(snapshot(&s), before);
        assert!(apply_truck_cargo_loss(&c, &mut s, &ids, 0, &[allocation(id)]).is_err());
        assert!(
            apply_truck_cargo_loss(
                &c,
                &mut s,
                &[id.clone(), id.clone()],
                0,
                &[allocation(id), allocation(id)]
            )
            .is_err()
        );
        assert_eq!(
            apply_truck_cargo_loss(&c, &mut s, &[], 99, &[]).unwrap(),
            TruckCargoLossReport::default()
        );
    }
}
