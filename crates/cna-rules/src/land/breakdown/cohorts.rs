//! Truck BP follows stable physical fuel cohorts, independently of unit-body exposure.
use super::{Asset, Equipment};
use crate::{
    State,
    logistics::{self, FuelTruckKind, TruckFuelCohort},
};
use cna_core::{engine::EngineError, ids::UnitId};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    pub base_quarters: i32,
    pub light_extra_quarters: i32,
    pub checked: Option<usize>,
}
fn gap() -> EngineError {
    EngineError::Invariant {
        detail: "truck breakdown cohort accounting is invalid".into(),
    }
}
fn fallback(s: &State, id: &UnitId) -> History {
    History {
        base_quarters: s
            .land
            .breakdown
            .accumulated_quarters
            .get(id)
            .copied()
            .unwrap_or(0),
        light_extra_quarters: s
            .land
            .breakdown
            .light_extra_quarters
            .get(id)
            .copied()
            .unwrap_or(0),
        checked: None,
    }
}
/// Initialize before recording an edge or transferring a physical cohort.
/// Cases: land:21.25, land:21.29
pub fn ensure(s: &mut State, id: &UnitId) -> Result<Vec<TruckFuelCohort>, EngineError> {
    let groups = logistics::segment_fuel_cohorts(s, id).map_err(|_| gap())?;
    for g in &groups {
        if s.land.breakdown.truck_histories.contains_key(&g.id) {
            continue;
        }
        let h = g
            .parent
            .as_ref()
            .and_then(|p| s.land.breakdown.truck_histories.get(p))
            .cloned()
            .unwrap_or_else(|| fallback(s, id));
        s.land.breakdown.truck_histories.insert(g.id.clone(), h);
    }
    Ok(groups)
}
/// A split copies its parent's exact BP and prior checked band; a whole transfer keeps its id.
/// Cases: land:21.25, land:21.26, land:21.29
pub fn inherit(s: &mut State, groups: &[TruckFuelCohort]) -> Result<(), EngineError> {
    for g in groups {
        if s.land.breakdown.truck_histories.contains_key(&g.id) {
            continue;
        }
        let h = g
            .parent
            .as_ref()
            .and_then(|p| s.land.breakdown.truck_histories.get(p))
            .cloned()
            .ok_or_else(gap)?;
        s.land.breakdown.truck_histories.insert(g.id.clone(), h);
    }
    Ok(())
}
pub(super) fn add(s: &mut State, id: &UnitId, base: i32, extra: i32) -> Result<(), EngineError> {
    let groups = ensure(s, id)?;
    for g in groups {
        let h = s.land.breakdown.truck_histories.get_mut(&g.id).unwrap();
        h.base_quarters = h.base_quarters.checked_add(base).ok_or_else(gap)?;
        if g.kind == FuelTruckKind::Light {
            h.light_extra_quarters = h.light_extra_quarters.checked_add(extra).ok_or_else(gap)?;
        }
    }
    Ok(())
}
pub(super) fn points(s: &State, a: &Asset) -> Option<i32> {
    let h = s.land.breakdown.truck_histories.get(a.cohort.as_ref()?)?;
    h.base_quarters
        .checked_add(if a.equipment == Equipment::LightTruck {
            h.light_extra_quarters
        } else {
            0
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CnaContent,
        land::breakdown::{self, Category},
        state::Location,
    };
    use cna_content::units::Trucks;
    use cna_tables::land::weather::WeatherKind;
    /// Cases: land:21.25, land:21.26, land:21.29
    #[test]
    fn transferred_trucks_keep_bp_and_checked_band_separate_from_receiving_body() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let from: UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
        let to = s
            .land
            .units
            .keys()
            .find(|id| {
                **id != from
                    && super::super::super::formation::class(&c, id)
                        .is_some_and(|cl| cl.unit_type == "infantry")
            })
            .unwrap()
            .clone();
        for id in [&from, &to] {
            let u = s.land.units.get_mut(id).unwrap();
            u.location = Location::Hex {
                hex: "C4020".into(),
            };
            u.trucks = Trucks {
                medium: 2,
                ..Trucks::default()
            };
            u.transport_trucks = Trucks::default();
        }
        breakdown::record_edge(&mut s, &from, &"C4020".into(), 80, 8, WeatherKind::Normal).unwrap();
        let donor = logistics::segment_fuel_cohorts(&s, &from).unwrap();
        s.land
            .breakdown
            .truck_histories
            .get_mut(&donor[0].id)
            .unwrap()
            .checked = Some(2);
        ensure(&mut s, &to).unwrap();
        let moved = logistics::transfer_selected_segment_fuel_cohorts(
            &mut s,
            &from,
            &to,
            &[logistics::FuelCohortSelection {
                id: donor[0].id.clone(),
                count: 1,
            }],
        )
        .unwrap();
        inherit(&mut s, &moved).unwrap();
        assert_eq!(moved[0].parent.as_ref(), Some(&donor[0].id));
        s.land.units.get_mut(&from).unwrap().trucks.medium -= 1;
        s.land.units.get_mut(&to).unwrap().trucks.medium += 1;
        breakdown::record_edge(&mut s, &to, &"C4020".into(), 4, 4, WeatherKind::Normal).unwrap();
        breakdown::stop(&mut s, std::slice::from_ref(&to), &"C4021".into());
        let groups = breakdown::check_groups(&c, &s, &s.land.breakdown.stopped[0]).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].category, Category::Truck);
        assert_eq!(groups[0].column, 3);
        assert_eq!(groups[0].assets[0].points, 1);
        assert_eq!(groups[0].assets[0].cohort.as_ref(), Some(&moved[0].id));
        assert_eq!(s.land.breakdown.accumulated_quarters[&to], 4);
        assert_eq!(
            s.land.breakdown.truck_histories[&moved[0].id].base_quarters,
            84
        );
    }
}
