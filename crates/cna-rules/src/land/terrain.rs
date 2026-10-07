//! TEC movement price for an explicitly classified hex and edge. Unknown is a loader concern.
use cna_tables::land::terrain::{TerrainEffects, TerrainFeature as F, TerrainValue as V};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Plain,
    Road,
    Track,
    UnfinishedRoad,
    Railroad,
    UnfinishedRailroad,
}

#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    pub terrain: F,
    pub route: Route,
    pub hexsides: &'a [F],
    pub rainstorm: bool,
    pub motorized: bool,
    pub salt_marsh_exception: bool,
    pub desert_prohibited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryError {
    Prohibited,
    MissingTerrain,
    WrongFeature,
    Overflow,
}

/// Costs are quarter CP. Completed railroads count as roads for crossing a rain-swollen wadi
/// or a major river, but do not grant road entry prices by themselves. Unfinished roads are tracks.
/// Motorized marsh exceptions and light-truck/motorcycle desert restrictions remain explicit.
/// Cases: land:8.31, land:8.32, land:8.33, land:8.35, land:8.37, land:8.41, land:8.42
/// Cases: land:8.43, land:8.44, land:8.45, land:8.46, land:8.47, land:8.48
/// Cases: land:29.55, land:29.56
/// Interpretations: interp:land-0002
pub fn entry_cost(table: &TerrainEffects, entry: Entry<'_>) -> Result<i32, EntryError> {
    let Entry {
        terrain,
        route,
        hexsides,
        rainstorm,
        motorized,
        salt_marsh_exception,
        desert_prohibited,
    } = entry;
    if matches!(
        terrain,
        F::Road | F::Track | F::Railroad | F::VillageBirOasis
    ) || !matches!(
        table.feature(terrain).group,
        cna_tables::land::terrain::FeatureGroup::HexTerrain
    ) {
        return Err(EntryError::WrongFeature);
    }
    if terrain == F::Desert && desert_prohibited {
        return Err(EntryError::Prohibited);
    }
    let road = route == Route::Road && !rainstorm;
    let real_track = matches!(route, Route::Track | Route::UnfinishedRoad);
    let track = real_track || (route == Route::Road && rainstorm);
    let bridge = route == Route::Road || route == Route::Railroad;
    // The printed marsh vehicle cell is for the limited vehicles allowed off the network.
    if terrain == F::SaltMarsh && motorized && !salt_marsh_exception && !road && !track {
        return Err(EntryError::Prohibited);
    }
    let cell = |feature| {
        let r = table.feature(feature);
        if motorized {
            r.cp_motorized
        } else {
            r.cp_non_motorized
        }
    };
    let entry_cell = if road {
        cell(F::Road)
    } else if track {
        table.track_values(terrain, motorized).0
    } else {
        cell(terrain)
    };
    let mut cost = match entry_cell {
        Some(V::EnterQuarters(n)) => n,
        Some(V::RoadOrRailOnly) if bridge => match cell(F::Road) {
            Some(V::EnterQuarters(n)) => n,
            _ => return Err(EntryError::MissingTerrain),
        },
        Some(V::Prohibited | V::RoadOrRailOnly) => return Err(EntryError::Prohibited),
        _ => return Err(EntryError::MissingTerrain),
    };
    for &edge in hexsides {
        if !matches!(
            table.feature(edge).group,
            cna_tables::land::terrain::FeatureGroup::HexsideFeature
        ) {
            return Err(EntryError::WrongFeature);
        }
        // Escarpments are a vehicle exception to road/track bypass of terrain features.
        if motorized && edge == F::UpEscarpment {
            return Err(EntryError::Prohibited);
        }
        if motorized && edge == F::DownEscarpment && !real_track {
            return Err(EntryError::Prohibited);
        }
        let extra = if edge == F::Wadi && rainstorm {
            if !bridge {
                return Err(EntryError::Prohibited);
            }
            2 * 4
        } else if road || (bridge && edge == F::MajorRiver) {
            0
        } else {
            let value = if track {
                table.track_values(edge, motorized).0
            } else {
                cell(edge)
            };
            match value {
                Some(V::AddQuarters(n)) => n,
                Some(V::NoEffect) => 0,
                Some(V::Prohibited | V::RoadOrRailOnly) => return Err(EntryError::Prohibited),
                _ => return Err(EntryError::MissingTerrain),
            }
        };
        cost = cost.checked_add(extra).ok_or(EntryError::Overflow)?;
    }
    Ok(cost)
}

/// Breakdown exposure follows the travelled terrain, independently of movement CPA.
/// Rain converts the road value to the track value; road bypass applies to hexside effects.
/// Cases: land:21.21, land:21.37, land:8.37
/// Interpretations: interp:land-0002
pub fn entry_breakdown(table: &TerrainEffects, entry: Entry<'_>) -> Result<i32, EntryError> {
    // Reuse crossing legality, including vehicle/escarpment and swollen-river restrictions.
    entry_cost(table, entry)?;
    let road = entry.route == Route::Road && !entry.rainstorm;
    let track = matches!(entry.route, Route::Track | Route::UnfinishedRoad)
        || entry.route == Route::Road && entry.rainstorm;
    let value = |f| {
        if track {
            table.track_values(f, entry.motorized).1
        } else {
            table.feature(f).breakdown
        }
    };
    let number = |v| match v {
        Some(V::ValueQuarters(n) | V::AddQuarters(n)) => Ok(n),
        None | Some(V::NoEffect) => Ok(0),
        _ => Err(EntryError::MissingTerrain),
    };
    let mut total = number(if road {
        table.feature(F::Road).breakdown
    } else {
        value(entry.terrain)
    })?;
    for &edge in entry.hexsides {
        if road || (matches!(entry.route, Route::Road | Route::Railroad) && edge == F::MajorRiver) {
            continue;
        }
        total = total
            .checked_add(number(value(edge))?)
            .ok_or(EntryError::Overflow)?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cost(
        terrain: F,
        route: Route,
        edge: Option<F>,
        motorized: bool,
        rainstorm: bool,
    ) -> Result<i32, EntryError> {
        let t = cna_tables::Tables::load(&cna_content::repo_data_dir()).unwrap();
        let edges: Vec<_> = edge.into_iter().collect();
        entry_cost(
            &t.land.terrain_effects,
            Entry {
                terrain,
                route,
                hexsides: &edges,
                motorized,
                rainstorm,
                salt_marsh_exception: false,
                desert_prohibited: false,
            },
        )
    }
    /// Cases: land:8.31, land:8.37, land:8.41, land:8.43, land:8.46, land:29.56
    /// Interpretations: interp:land-0002
    #[test]
    fn hand_checked_plain_road_track_and_wadi_prices_are_exact() {
        assert_eq!(cost(F::Clear, Route::Plain, None, true, false), Ok(8));
        assert_eq!(cost(F::Clear, Route::Road, None, true, false), Ok(2));
        assert_eq!(cost(F::Clear, Route::Track, None, true, false), Ok(4));
        assert_eq!(cost(F::Clear, Route::Plain, None, false, false), Ok(8));
        assert_eq!(cost(F::Clear, Route::Road, None, false, false), Ok(4));
        assert_eq!(cost(F::Clear, Route::Track, None, false, false), Ok(4));
        assert_eq!(
            cost(F::Clear, Route::Track, Some(F::Wadi), true, false),
            Ok(12)
        );
        assert_eq!(
            cost(F::Clear, Route::Road, Some(F::Wadi), true, true),
            Ok(12)
        );
        assert_eq!(
            cost(F::Clear, Route::Track, Some(F::Wadi), true, true),
            Err(EntryError::Prohibited)
        );
        assert_eq!(cost(F::MajorCity, Route::Track, None, true, false), Ok(1));
    }
    /// Cases: land:8.31, land:8.43, land:8.45
    #[test]
    fn multiple_hexside_features_add_and_invalid_input_never_becomes_free() {
        let t = cna_tables::Tables::load(&cna_content::repo_data_dir()).unwrap();
        let mut entry = Entry {
            terrain: F::Clear,
            route: Route::Plain,
            hexsides: &[F::Ridge, F::Wadi],
            rainstorm: false,
            motorized: true,
            salt_marsh_exception: false,
            desert_prohibited: false,
        };
        // Clear 2 CP + ridge 4 CP + wadi 4 CP.
        assert_eq!(entry_cost(&t.land.terrain_effects, entry), Ok(40));
        entry.hexsides = &[F::MajorCity];
        assert_eq!(
            entry_cost(&t.land.terrain_effects, entry),
            Err(EntryError::WrongFeature)
        );
        entry.hexsides = &[];
        entry.terrain = F::Desert;
        entry.route = Route::Track;
        entry.desert_prohibited = true;
        assert_eq!(
            entry_cost(&t.land.terrain_effects, entry),
            Err(EntryError::Prohibited)
        );
    }

    /// Cases: land:21.21, land:21.37, land:8.37
    /// Interpretations: interp:land-0002
    #[test]
    fn hand_checked_breakdown_prices_include_track_quarters_and_rain() {
        let t = cna_tables::Tables::load(&cna_content::repo_data_dir()).unwrap();
        let mut e = Entry {
            terrain: F::Clear,
            route: Route::Plain,
            hexsides: &[F::Wadi],
            rainstorm: false,
            motorized: true,
            salt_marsh_exception: false,
            desert_prohibited: false,
        };
        assert_eq!(entry_breakdown(&t.land.terrain_effects, e), Ok(48)); // 4 + 8 BP
        e.route = Route::Track;
        assert_eq!(entry_breakdown(&t.land.terrain_effects, e), Ok(24)); // 2 + 4 BP
        e.route = Route::Road;
        assert_eq!(entry_breakdown(&t.land.terrain_effects, e), Ok(2));
        e.rainstorm = true;
        assert_eq!(entry_breakdown(&t.land.terrain_effects, e), Ok(24));
        e.hexsides = &[];
        e.terrain = F::MajorCity;
        e.route = Route::Track;
        e.rainstorm = false;
        assert_eq!(entry_breakdown(&t.land.terrain_effects, e), Ok(1)); // 0.25 BP
        e.route = Route::Railroad;
        e.terrain = F::Clear;
        assert_eq!(entry_breakdown(&t.land.terrain_effects, e), Ok(16));
    }
    /// Cases: land:8.32, land:8.42, land:8.44, land:8.47
    #[test]
    fn network_does_not_bypass_vehicle_escarpment_restrictions() {
        assert_eq!(
            cost(F::Clear, Route::Road, Some(F::UpEscarpment), true, false),
            Err(EntryError::Prohibited)
        );
        assert_eq!(
            cost(F::Clear, Route::Plain, Some(F::DownEscarpment), true, false),
            Err(EntryError::Prohibited)
        );
        assert_eq!(
            cost(F::Clear, Route::Track, Some(F::DownEscarpment), true, false),
            Ok(36)
        );
        assert_eq!(
            cost(F::SaltMarsh, Route::Plain, None, true, false),
            Err(EntryError::Prohibited)
        );
        assert!(cost(F::SaltMarsh, Route::Track, None, true, false).is_ok());
        assert_eq!(
            cost(F::Clear, Route::UnfinishedRoad, None, true, false),
            Ok(4)
        );
    }
}
