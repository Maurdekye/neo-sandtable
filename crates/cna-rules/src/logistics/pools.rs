//! Stable truck-convoy identities. Cargo is stored on its pool, never by vector position.
use crate::state::{Location, LogisticsState, TruckPool};
use cna_content::{
    scenario::{Placement, Supplies},
    units::Trucks,
};
use cna_protocol::Side;

/// Create a pool with a never-reused identity. A split must create a new pool and
/// explicitly divide trucks and cargo; this helper never copies a parent's cargo.
/// Cases: airlog:53.11, airlog:53.24, scen:59.41
pub fn add_truck_pool(
    logistics: &mut LogisticsState,
    source_id: Option<&str>,
    side: Side,
    placement: Placement,
    location: Option<Location>,
    trucks: Trucks,
    cargo: Supplies,
) -> Result<String, String> {
    if [
        trucks.light,
        trucks.medium,
        trucks.heavy,
        cargo.ammo,
        cargo.fuel,
        cargo.stores,
        cargo.water,
    ]
    .into_iter()
    .any(|n| n < 0)
        || location
            .as_ref()
            .is_some_and(|l| !matches!(l, Location::Hex { .. } | Location::OffMap { .. }))
    {
        return Err("invalid truck-pool holdings or location".into());
    }
    let used = |id: &str| {
        logistics.truck_pool_ids.contains(id) || logistics.truck_pools.iter().any(|p| p.id == id)
    };
    let mut serial = logistics
        .truck_pool_serial
        .get(&side)
        .copied()
        .unwrap_or_default();
    let prefix = format!("{}.pool-", crate::state::side_key(side));
    let id = if let Some(id) = source_id {
        if id.is_empty() || used(id) {
            return Err("truck-pool id is empty or already used".into());
        }
        if let Some(n) = id.strip_prefix(&prefix).and_then(|s| s.parse::<u64>().ok()) {
            serial = serial.max(n);
        }
        id.to_owned()
    } else {
        loop {
            serial = serial
                .checked_add(1)
                .ok_or("truck-pool identity space exhausted")?;
            let id = format!("{prefix}{serial}");
            if !used(&id) {
                break id;
            }
        }
    };
    logistics.truck_pool_serial.insert(side, serial);
    logistics.truck_pool_ids.insert(id.clone());
    logistics.truck_pools.push(TruckPool {
        id: id.clone(),
        side,
        placement,
        location,
        trucks,
        cargo,
    });
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CnaContent, State};
    use cna_core::visibility::Perspective;

    fn add(
        state: &mut LogisticsState,
        id: Option<&str>,
        cargo: Supplies,
    ) -> Result<String, String> {
        add_truck_pool(
            state,
            id,
            Side::Axis,
            Placement::City {
                city: "tripoli".into(),
            },
            Some(Location::OffMap {
                id: "box_tripoli".into(),
            }),
            Trucks {
                light: 1,
                ..Trucks::default()
            },
            cargo,
        )
    }
    /// Cases: airlog:53.11, scen:59.41
    #[test]
    fn removal_reordering_and_checkpoint_do_not_move_cargo_or_reuse_ids() {
        let mut logistics = LogisticsState::default();
        let first = add(
            &mut logistics,
            None,
            Supplies {
                fuel: 10,
                ..Supplies::default()
            },
        )
        .unwrap();
        let second = add(
            &mut logistics,
            None,
            Supplies {
                fuel: 20,
                ..Supplies::default()
            },
        )
        .unwrap();
        logistics.truck_pools.swap(0, 1);
        assert_eq!(
            logistics
                .truck_pools
                .iter()
                .find(|p| p.id == first)
                .unwrap()
                .cargo
                .fuel,
            10
        );
        logistics.truck_pools.retain(|p| p.id != first);
        let mut restored: LogisticsState =
            serde_json::from_value(serde_json::to_value(&logistics).unwrap()).unwrap();
        let third = add(&mut restored, None, Supplies::default()).unwrap();
        assert_ne!(first, third);
        assert_ne!(second, third);
        assert_eq!(third, "axis.pool-3");
        let before = serde_json::to_value(&restored).unwrap();
        assert!(add(&mut restored, Some(&first), Supplies::default()).is_err());
        assert_eq!(serde_json::to_value(&restored).unwrap(), before);
        let source = add(&mut restored, Some("source-pool"), Supplies::default()).unwrap();
        assert_eq!(source, "source-pool");
        assert!(add(&mut restored, Some("source-pool"), Supplies::default()).is_err());
    }
    /// Cases: airlog:49.3, airlog:52.44, land:29.34
    #[test]
    fn cargo_evaporation_follows_pool_location_and_leaves_boxes_unchanged() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut state = State::new(&content).unwrap();
        let side = Side::Axis;
        let cargo = Supplies {
            fuel: 100,
            water: 100,
            ..Supplies::default()
        };
        let on_map = add_truck_pool(
            &mut state.logistics,
            None,
            side,
            Placement::Hex {
                hex: "C4020".into(),
            },
            Some(Location::Hex {
                hex: "C4020".into(),
            }),
            Trucks {
                heavy: 10,
                ..Trucks::default()
            },
            cargo,
        )
        .unwrap();
        let off_map = add(&mut state.logistics, None, cargo).unwrap();
        crate::logistics::stores::weekly_losses(&content, &mut state);
        assert_eq!(
            state
                .logistics
                .truck_pools
                .iter()
                .find(|p| p.id == on_map)
                .unwrap()
                .cargo
                .fuel,
            94
        );
        let seed = (0..=255)
            .find(|seed| {
                content.tables.land.weather.result(
                    1,
                    cna_core::dice::CampaignRng::from_seed([*seed; 32]).two_dice_reading(),
                ) == Some(cna_tables::land::weather::WeatherKind::Hot)
            })
            .unwrap();
        let mut rng = cna_core::dice::CampaignRng::from_seed([seed; 32]);
        let mut events = Vec::new();
        crate::logistics::weather::determine(
            &content,
            &mut state,
            &mut cna_core::engine::Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        let loaded = &state
            .logistics
            .truck_pools
            .iter()
            .find(|p| p.id == on_map)
            .unwrap()
            .cargo;
        assert_eq!(loaded.fuel, 90);
        assert_eq!(loaded.water, 90);
        assert_eq!(
            state
                .logistics
                .truck_pools
                .iter()
                .find(|p| p.id == off_map)
                .unwrap()
                .cargo,
            cargo
        );
    }
    /// Cases: scen:60.33, scen:60.43, land:3.6
    #[test]
    fn real_scenario_ids_are_deterministic_and_pool_details_are_owner_only() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut state = State::new(&content).unwrap();
        assert_eq!(
            state.logistics.truck_pools,
            State::new(&content).unwrap().logistics.truck_pools
        );
        assert!(state.logistics.truck_pools.iter().all(|p| !p.id.is_empty()));
        let pool = state
            .logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.side == Side::Axis)
            .unwrap();
        pool.cargo.ammo = 37;
        let id = pool.id.clone();
        let own = crate::view::inspect(&content, &state, Perspective::Side(Side::Axis), &id, false)
            .unwrap();
        assert_eq!(own["truck_pool"]["cargo"]["ammo"], 37);
        assert!(
            crate::view::inspect(
                &content,
                &state,
                Perspective::Side(Side::Commonwealth),
                &id,
                false
            )
            .is_err()
        );
        let enemy = crate::view::observe(&content, &state, Perspective::Side(Side::Commonwealth));
        assert!(
            enemy["logistics"]["truck_pools"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["side"] != "axis")
        );
        let mut old = own["truck_pool"].clone();
        old.as_object_mut().unwrap().remove("cargo");
        old.as_object_mut().unwrap().remove("location");
        let compatible: TruckPool = serde_json::from_value(old).unwrap();
        assert_eq!(compatible.cargo, Supplies::default());
        assert!(compatible.location.is_none());
    }
}
