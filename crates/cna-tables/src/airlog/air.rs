//! Air-game tables that are not two-dice-reading combat charts: pilots, squadrons, facilities,
//! refit, missions, scramble, mining, reconnaissance, Malta, and air-to-air combat
//! (sections 34-45).

use std::ops::Deref;

use cna_core::dice::{Die, TwoDiceReading};
use serde::Deserialize;

use crate::calendar::Month;
use crate::ranges::{Band, IntRange, band_index, check_bands, check_int_tiling, is_reading};
use crate::{Bound, RawTable, TableError};

// ---------------------------------------------------------------------------------------------
// 34.86 / 34.89a / 34.89b Pilot Arrival
// ---------------------------------------------------------------------------------------------

/// A calendar month in a year, written `"1940-11"` in the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct YearMonth {
    pub year: i32,
    pub month: Month,
}

impl YearMonth {
    fn parse(s: &str) -> Option<Self> {
        let (y, m) = s.split_once('-')?;
        Some(Self {
            year: y.parse().ok()?,
            month: Month::from_number(m.parse().ok()?)?,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RollMonthsRaw {
    first: String,
    last: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PilotRowRaw {
    dice_total: i32,
    pilots: Vec<i32>,
}

#[derive(Debug, Clone, Deserialize)]
struct PilotExtraRaw {
    game_turn: i32,
    pilot_rating: i32,
    count: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct PilotRaw {
    roll_months: RollMonthsRaw,
    pilot_ratings: Vec<i32>,
    row: Vec<PilotRowRaw>,
    #[serde(default)]
    extra: Option<PilotExtraRaw>,
}

/// Pilots received on one roll, one count per rating (`pilot_ratings`, One to Four).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PilotsReceived {
    /// `(rating, count)` for each rating column.
    pub by_rating: Vec<(i32, i32)>,
}

impl PilotsReceived {
    /// Pilots of one rating (0 for a rating the chart has no column for).
    pub fn count(&self, rating: i32) -> i32 {
        self.by_rating
            .iter()
            .find(|(r, _)| *r == rating)
            .map_or(0, |(_, c)| *c)
    }

    pub fn total(&self) -> i32 {
        self.by_rating.iter().map(|(_, c)| *c).sum()
    }
}

/// A pilot extra that arrives on a fixed Game-Turn instead of by a roll (German ace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledPilot {
    pub game_turn: i32,
    pub pilot_rating: i32,
    pub count: i32,
}

/// One nationality's Pilot Arrival Table.
#[derive(Debug, Clone)]
pub struct PilotArrival {
    first: YearMonth,
    last: YearMonth,
    ratings: Vec<i32>,
    rows: Vec<(i32, Vec<i32>)>,
    extra: Option<ScheduledPilot>,
}

impl PilotArrival {
    fn from_raw_table(raw: &RawTable) -> Result<Self, TableError> {
        let r: PilotRaw = raw.deserialize()?;
        if r.pilot_ratings.is_empty()
            || r.pilot_ratings.iter().any(|v| !(1..=4).contains(v))
            || r.pilot_ratings.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(raw.err(
                "pilot_ratings",
                "ratings must be distinct, ascending, and within 1-4",
            ));
        }
        if r.extra
            .as_ref()
            .is_some_and(|e| e.game_turn < 1 || !(1..=6).contains(&e.pilot_rating) || e.count < 1)
        {
            return Err(raw.err(
                "extra",
                "needs a positive game turn and count, and rating 1-6",
            ));
        }
        let month = |field: &str, s: &str| {
            YearMonth::parse(s)
                .ok_or_else(|| raw.err(format!("roll_months.{field}"), "expected `YYYY-MM`"))
        };
        let first = month("first", &r.roll_months.first)?;
        let last = month("last", &r.roll_months.last)?;
        if first > last {
            return Err(raw.err("roll_months", "first is after last"));
        }
        check_int_tiling(
            r.row
                .iter()
                .map(|x| IntRange::new(x.dice_total, x.dice_total)),
            2,
            12,
        )
        .map_err(|m| raw.err("row.dice_total", m))?;
        for (i, row) in r.row.iter().enumerate() {
            if row.pilots.len() != r.pilot_ratings.len() {
                return Err(raw.err(
                    format!("row[{i}].pilots"),
                    "needs one count per pilot rating",
                ));
            }
            if row.pilots.iter().any(|&c| c < 0) {
                return Err(raw.err(format!("row[{i}].pilots"), "counts must not be negative"));
            }
        }
        Ok(Self {
            first,
            last,
            ratings: r.pilot_ratings,
            rows: r
                .row
                .into_iter()
                .map(|x| (x.dice_total, x.pilots))
                .collect(),
            extra: r.extra.map(|e| ScheduledPilot {
                game_turn: e.game_turn,
                pilot_rating: e.pilot_rating,
                count: e.count,
            }),
        })
    }

    /// Pilots received for a two-dice total (2-12). `airlog:34.8`.
    pub fn pilots(&self, dice_total: i32) -> Option<PilotsReceived> {
        let (_, counts) = self.rows.iter().find(|(t, _)| *t == dice_total)?;
        Some(PilotsReceived {
            by_rating: self
                .ratings
                .iter()
                .copied()
                .zip(counts.iter().copied())
                .collect(),
        })
    }

    /// Whether the nationality rolls for pilots in the given month. `airlog:34.8`.
    pub fn rolls_in(&self, year: i32, month: Month) -> bool {
        let m = YearMonth { year, month };
        self.first <= m && m <= self.last
    }

    /// The first and last months in which pilots are rolled for.
    pub fn roll_window(&self) -> (YearMonth, YearMonth) {
        (self.first, self.last)
    }

    /// A pilot that arrives on a fixed Game-Turn, if the table has one.
    pub fn scheduled_extra(&self) -> Option<ScheduledPilot> {
        self.extra
    }
}

macro_rules! pilot_table {
    ($(#[$meta:meta])* $name:ident, $id:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $name(pub PilotArrival);

        impl Deref for $name {
            type Target = PilotArrival;
            fn deref(&self) -> &PilotArrival {
                &self.0
            }
        }

        impl Bound for $name {
            const ID: &'static str = $id;
            fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
                PilotArrival::from_raw_table(raw).map(Self)
            }
        }
    };
}

pilot_table!(
    /// Commonwealth Pilot Arrival Table (`airlog:34.86`; the text calls it 34.88).
    CommonwealthPilotArrival,
    "airlog.34.86.commonwealth_pilot_arrival"
);
pilot_table!(
    /// Italian Pilot Arrival Table (`airlog:34.89`).
    ItalianPilotArrival,
    "airlog.34.89a.italian_pilot_arrival"
);
pilot_table!(
    /// German Pilot Arrival Table (`airlog:34.89`).
    GermanPilotArrival,
    "airlog.34.89b.german_pilot_arrival"
);

// ---------------------------------------------------------------------------------------------
// 35.23 Squadron Capacity
// ---------------------------------------------------------------------------------------------

/// The squadron kinds of the capacity chart (`airlog:35.23`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadronKind {
    ItalianSquadriglia,
    GermanStaffel,
    #[serde(rename = "commonwealth_squadron_1940_41")]
    CommonwealthSquadron194041,
    #[serde(rename = "commonwealth_squadron_1942_43")]
    CommonwealthSquadron194243,
}

/// Planes a squadron holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SquadronCapacity {
    pub squadron: SquadronKind,
    pub ready: i32,
    pub reserve: i32,
    pub total: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct SquadronRaw {
    row: Vec<SquadronCapacity>,
}

/// The Squadron Capacity Chart (`airlog:35.23`).
#[derive(Debug, Clone)]
pub struct SquadronCapacityTable {
    rows: Vec<SquadronCapacity>,
}

impl Bound for SquadronCapacityTable {
    const ID: &'static str = "airlog.35.23.squadron_capacity";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: SquadronRaw = raw.deserialize()?;
        for (i, row) in r.row.iter().enumerate() {
            if row.ready < 0
                || row.reserve < 0
                || row.ready.checked_add(row.reserve) != Some(row.total)
            {
                return Err(raw.err(
                    format!("row[{i}].total"),
                    "total must equal ready plus reserve",
                ));
            }
        }
        for k in [
            SquadronKind::ItalianSquadriglia,
            SquadronKind::GermanStaffel,
            SquadronKind::CommonwealthSquadron194041,
            SquadronKind::CommonwealthSquadron194243,
        ] {
            if r.row.iter().filter(|x| x.squadron == k).count() != 1 {
                return Err(raw.err("row", format!("{k:?} must appear exactly once")));
            }
        }
        Ok(Self { rows: r.row })
    }
}

impl SquadronCapacityTable {
    /// Planes a squadron of this kind holds. `airlog:35.23`.
    pub fn capacity(&self, kind: SquadronKind) -> SquadronCapacity {
        *self
            .rows
            .iter()
            .find(|x| x.squadron == kind)
            .expect("validated: every kind present")
    }
}

// ---------------------------------------------------------------------------------------------
// 36.53 Allied Off-Map Air Facilities
// ---------------------------------------------------------------------------------------------

/// The Commonwealth off-map air facilities (`airlog:36.53`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OffMapFacility {
    PortSaid,
    AbuSeier,
    Ismailia,
    Fayid,
    Deversoir,
    Kabrit,
    Ethiopia,
}

/// How many squadrons a facility can base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SquadronLimit {
    Squadrons(i32),
    Unlimited,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum LimitRaw {
    Count(i32),
    Word(String),
}

/// The first OpStage from which a facility may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct AvailableFrom {
    pub game_turn: i32,
    pub opstage: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct FacilityRowRaw {
    entry_exit_hex: String,
    facility: OffMapFacility,
    #[serde(default)]
    facility_type: Option<String>,
    distance_hexes: i32,
    max_squadrons: LimitRaw,
    #[serde(default)]
    available_from: Option<AvailableFrom>,
}

#[derive(Debug, Clone, Deserialize)]
struct FacilityRaw {
    row: Vec<FacilityRowRaw>,
}

/// One row of the off-map facilities chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffMapAirFacility {
    pub facility: OffMapFacility,
    /// The map hex where planes enter or leave the map.
    pub entry_exit_hex: String,
    pub facility_type: Option<String>,
    pub distance_hexes: i32,
    pub max_squadrons: SquadronLimit,
    pub available_from: Option<AvailableFrom>,
}

/// The Allied Off-Map Air Facilities chart (`airlog:36.53`).
#[derive(Debug, Clone)]
pub struct OffMapAirFacilities {
    rows: Vec<OffMapAirFacility>,
}

impl Bound for OffMapAirFacilities {
    const ID: &'static str = "airlog.36.53.allied_offmap_air_facilities";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: FacilityRaw = raw.deserialize()?;
        if r.row.len() != 7 {
            return Err(raw.err("row", "needs all seven off-map facilities"));
        }
        let mut rows: Vec<OffMapAirFacility> = Vec::new();
        for (i, row) in r.row.into_iter().enumerate() {
            if row.distance_hexes <= 0 {
                return Err(raw.err(format!("row[{i}].distance_hexes"), "must be positive"));
            }
            if row
                .available_from
                .is_some_and(|v| v.game_turn < 1 || !(1..=3).contains(&v.opstage))
            {
                return Err(raw.err(
                    format!("row[{i}].available_from"),
                    "needs a positive game turn and opstage 1-3",
                ));
            }
            let max_squadrons = match row.max_squadrons {
                LimitRaw::Count(n) if n > 0 => SquadronLimit::Squadrons(n),
                LimitRaw::Word(w) if w == "unlimited" => SquadronLimit::Unlimited,
                _ => {
                    return Err(raw.err(
                        format!("row[{i}].max_squadrons"),
                        "must be a positive count or \"unlimited\"",
                    ));
                }
            };
            if rows.iter().any(|x| x.facility == row.facility) {
                return Err(raw.err(format!("row[{i}].facility"), "facility listed twice"));
            }
            rows.push(OffMapAirFacility {
                facility: row.facility,
                entry_exit_hex: row.entry_exit_hex,
                facility_type: row.facility_type,
                distance_hexes: row.distance_hexes,
                max_squadrons,
                available_from: row.available_from,
            });
        }
        Ok(Self { rows })
    }
}

impl OffMapAirFacilities {
    /// The row for a facility. `airlog:36.53`.
    pub fn facility(&self, f: OffMapFacility) -> &OffMapAirFacility {
        self.rows
            .iter()
            .find(|x| x.facility == f)
            .expect("validated: the chart lists every facility it names")
    }

    /// Every facility, in chart order.
    pub fn all(&self) -> &[OffMapAirFacility] {
        &self.rows
    }
}

// ---------------------------------------------------------------------------------------------
// 38.37 Aircraft Refit
// ---------------------------------------------------------------------------------------------

/// A nationality of air force (`airlog:38.37`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Nationality {
    Commonwealth,
    German,
    Italian,
}

#[derive(Debug, Clone, Deserialize)]
struct ByPlaneRaw {
    nationality: Nationality,
    refit_on_dice_total_range: IntRange,
}

#[derive(Debug, Clone, Deserialize)]
struct ByPlaneModifierRaw {
    planes_not_assigned_to_refitting_sgsu: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct BySquadronRaw {
    die_range: IntRange,
    percent_refitted: i32,
}

/// Die-roll modifiers of the per-squadron refit method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SquadronRefitModifiers {
    pub german_sgsu: i32,
    pub italian_sgsu: i32,
    pub planes_not_assigned_to_refitting_sgsu: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct RefitRaw {
    by_plane: Vec<ByPlaneRaw>,
    by_plane_modifier: ByPlaneModifierRaw,
    by_squadron: Vec<BySquadronRaw>,
    by_squadron_modifier: SquadronRefitModifiers,
}

/// The Aircraft Refit Table (`airlog:38.37`; the text calls it 38.38).
#[derive(Debug, Clone)]
pub struct AircraftRefit {
    by_plane: Vec<(Nationality, IntRange)>,
    by_plane_unassigned: i32,
    by_squadron: Vec<(IntRange, i32)>,
    squadron_modifiers: SquadronRefitModifiers,
}

impl Bound for AircraftRefit {
    const ID: &'static str = "airlog.38.37.aircraft_refit";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: RefitRaw = raw.deserialize()?;
        for n in [
            Nationality::Commonwealth,
            Nationality::German,
            Nationality::Italian,
        ] {
            if r.by_plane.iter().filter(|x| x.nationality == n).count() != 1 {
                return Err(raw.err("by_plane", format!("{n:?} must appear exactly once")));
            }
        }
        for (i, row) in r.by_plane.iter().enumerate() {
            if row.refit_on_dice_total_range.lo < 2 || row.refit_on_dice_total_range.hi > 12 {
                return Err(raw.err(
                    format!("by_plane[{i}].refit_on_dice_total_range"),
                    "must lie within 2-12",
                ));
            }
        }
        // One die plus the largest nationality and assignment modifiers.
        let last = 6
            + r.by_squadron_modifier
                .german_sgsu
                .max(r.by_squadron_modifier.italian_sgsu)
                .max(0)
            + r.by_squadron_modifier
                .planes_not_assigned_to_refitting_sgsu
                .max(0);
        check_int_tiling(r.by_squadron.iter().map(|x| x.die_range), 1, last)
            .map_err(|m| raw.err("by_squadron.die_range", m))?;
        if r.by_squadron
            .iter()
            .any(|x| !(0..=100).contains(&x.percent_refitted))
        {
            return Err(raw.err("by_squadron.percent_refitted", "must be 0-100"));
        }
        Ok(Self {
            by_plane: r
                .by_plane
                .into_iter()
                .map(|x| (x.nationality, x.refit_on_dice_total_range))
                .collect(),
            by_plane_unassigned: r.by_plane_modifier.planes_not_assigned_to_refitting_sgsu,
            by_squadron: r
                .by_squadron
                .into_iter()
                .map(|x| (x.die_range, x.percent_refitted))
                .collect(),
            squadron_modifiers: r.by_squadron_modifier,
        })
    }
}

impl AircraftRefit {
    /// Whether one plane is refitted by the per-plane method: two dice are rolled, two is added
    /// when the plane is not assigned to the SGSU doing the refit, and the plane is refitted
    /// when the total is within the nationality range. `airlog:38.37`.
    pub fn plane_refitted(
        &self,
        nationality: Nationality,
        dice_total: i32,
        assigned_to_refitting_sgsu: bool,
    ) -> bool {
        let range = self
            .by_plane
            .iter()
            .find(|(n, _)| *n == nationality)
            .map(|(_, r)| *r)
            .expect("validated: every nationality present");
        let modified = dice_total
            + if assigned_to_refitting_sgsu {
                0
            } else {
                self.by_plane_unassigned
            };
        range.contains(modified)
    }

    /// Percentage of a squadron planes refitted at a modified die roll (standard method);
    /// `None` above the last printed roll. `airlog:38.37`.
    pub fn squadron_percent_refitted(&self, modified_die: i32) -> Option<i32> {
        self.by_squadron
            .iter()
            .find(|(r, _)| r.contains(modified_die))
            .map(|(_, p)| *p)
    }

    /// The die-roll modifiers of the per-squadron method. `airlog:38.37`.
    pub fn squadron_modifiers(&self) -> SquadronRefitModifiers {
        self.squadron_modifiers
    }

    /// The modifier for planes not assigned to the SGSU in the per-plane method.
    pub fn plane_unassigned_modifier(&self) -> i32 {
        self.by_plane_unassigned
    }
}

// ---------------------------------------------------------------------------------------------
// 39.5 Aircraft Mission Summary
// ---------------------------------------------------------------------------------------------

/// Family of an air mission (`airlog:39.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionFamily {
    Strategic,
    LandSupport,
}

/// The side flying a strategic mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionSide {
    Commonwealth,
    Axis,
}

/// Class of a land-support mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionClass {
    Fighter,
    Bombing,
    NonCombat,
}

/// One row of the Aircraft Mission Summary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Mission {
    pub family: MissionFamily,
    #[serde(default)]
    pub side: Option<MissionSide>,
    #[serde(default)]
    pub class: Option<MissionClass>,
    /// Mission name as the data spells it, e.g. `bomb_ports`.
    pub mission: String,
    /// The rule case describing the mission, where the chart gives one.
    #[serde(default)]
    pub case: Option<String>,
    /// The mission may be flown at night.
    #[serde(default)]
    pub night: bool,
    #[serde(default)]
    pub resolved_on: Option<String>,
    #[serde(default)]
    pub restriction: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct MissionsRaw {
    mission: Vec<Mission>,
}

/// The Aircraft Mission Summary (`airlog:39.5`).
#[derive(Debug, Clone)]
pub struct MissionSummary {
    missions: Vec<Mission>,
}

impl Bound for MissionSummary {
    const ID: &'static str = "airlog.39.5.aircraft_mission_summary";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: MissionsRaw = raw.deserialize()?;
        for (i, m) in r.mission.iter().enumerate() {
            let ok = match m.family {
                MissionFamily::Strategic => m.side.is_some() && m.class.is_none(),
                MissionFamily::LandSupport => m.side.is_none() && m.class.is_some(),
            };
            if !ok {
                return Err(raw.err(
                    format!("mission[{i}]"),
                    "strategic missions need a side and no class; land-support missions need a class and no side",
                ));
            }
            if r.mission[..i]
                .iter()
                .any(|p| p.mission == m.mission && p.side == m.side)
            {
                return Err(raw.err(format!("mission[{i}].mission"), "mission listed twice"));
            }
        }
        Ok(Self {
            missions: r.mission,
        })
    }
}

impl MissionSummary {
    /// Every mission, in chart order.
    pub fn missions(&self) -> &[Mission] {
        &self.missions
    }

    /// A land-support mission by name. `airlog:39.5`.
    pub fn land_support(&self, name: &str) -> Option<&Mission> {
        self.missions
            .iter()
            .find(|m| m.family == MissionFamily::LandSupport && m.mission == name)
    }

    /// A strategic mission by side and name. `airlog:39.5`.
    pub fn strategic(&self, side: MissionSide, name: &str) -> Option<&Mission> {
        self.missions
            .iter()
            .find(|m| m.side == Some(side) && m.mission == name)
    }
}

// ---------------------------------------------------------------------------------------------
// 40.4 Scramble
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct ScrambleRowRaw {
    distance_hexes: IntRange,
    scramble_on_die_at_most: u8,
}

#[derive(Debug, Clone, Deserialize)]
struct ScrambleRaw {
    row: Vec<ScrambleRowRaw>,
}

/// The Scramble Table (`airlog:40.4`).
#[derive(Debug, Clone)]
pub struct Scramble {
    rows: Vec<(IntRange, u8)>,
}

impl Bound for Scramble {
    const ID: &'static str = "airlog.40.4.scramble";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: ScrambleRaw = raw.deserialize()?;
        let last = r.row.iter().map(|x| x.distance_hexes.hi).max().unwrap_or(0);
        check_int_tiling(r.row.iter().map(|x| x.distance_hexes), 0, last)
            .map_err(|m| raw.err("row.distance_hexes", m))?;
        if r.row
            .iter()
            .any(|x| !(1..=6).contains(&x.scramble_on_die_at_most))
        {
            return Err(raw.err("row.scramble_on_die_at_most", "must be 1-6"));
        }
        Ok(Self {
            rows: r
                .row
                .into_iter()
                .map(|x| (x.distance_hexes, x.scramble_on_die_at_most))
                .collect(),
        })
    }
}

impl Scramble {
    /// Highest die roll that scrambles a squadron whose base is `distance_hexes` from the target
    /// hex; `None` beyond the table (no scramble possible). `airlog:40.4`.
    pub fn scramble_on_die_at_most(&self, distance_hexes: i32) -> Option<u8> {
        self.rows
            .iter()
            .find(|(r, _)| r.contains(distance_hexes))
            .map(|(_, t)| *t)
    }

    /// Whether `die` scrambles the squadron. `airlog:40.4`.
    pub fn scrambles(&self, distance_hexes: i32, die: Die) -> bool {
        self.scramble_on_die_at_most(distance_hexes)
            .is_some_and(|t| die.value() <= t)
    }
}

// ---------------------------------------------------------------------------------------------
// 41.39 Mining Harbor by Plane
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct MiningRowRaw {
    average_bombload: Band,
    mined_on_die_at_most: u8,
}

#[derive(Debug, Clone, Deserialize)]
struct MiningRaw {
    row: Vec<MiningRowRaw>,
}

/// The Mining Harbor by Plane table (`airlog:41.39`).
#[derive(Debug, Clone)]
pub struct MiningHarbor {
    bands: Vec<Band>,
    thresholds: Vec<u8>,
}

impl Bound for MiningHarbor {
    const ID: &'static str = "airlog.41.39.mining_harbor_by_plane";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: MiningRaw = raw.deserialize()?;
        let bands: Vec<Band> = r.row.iter().map(|x| x.average_bombload).collect();
        check_bands(&bands, 0).map_err(|m| raw.err("row.average_bombload", m))?;
        if r.row.iter().any(|x| x.mined_on_die_at_most > 6) {
            return Err(raw.err("row.mined_on_die_at_most", "must be 0-6"));
        }
        Ok(Self {
            bands,
            thresholds: r.row.iter().map(|x| x.mined_on_die_at_most).collect(),
        })
    }
}

impl MiningHarbor {
    /// Highest die roll that mines the harbor for a group of six planes with this average
    /// bombload (0 means it cannot succeed). `airlog:41.39`.
    pub fn mined_on_die_at_most(&self, average_bombload: i32) -> u8 {
        band_index(&self.bands, average_bombload).map_or(0, |i| self.thresholds[i])
    }

    /// Whether `die` mines the harbor. `airlog:41.39`.
    pub fn mines(&self, average_bombload: i32, die: Die) -> bool {
        die.value() <= self.mined_on_die_at_most(average_bombload)
    }
}

// ---------------------------------------------------------------------------------------------
// 42.27 Air Recon of Land Units
// ---------------------------------------------------------------------------------------------

/// What a land-units reconnaissance reveals in a hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    /// This many battalion-equivalents of the units in the hex.
    BattalionEquivalents(i32),
    /// Every unit in the hex, whatever its size.
    All,
}

#[derive(Debug, Clone, Deserialize)]
struct ReconModifierRaw {
    per_planes: i32,
    add: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct ReconRowRaw {
    #[serde(default)]
    modified_roll: Option<IntRange>,
    #[serde(default)]
    modified_roll_min: Option<i32>,
    #[serde(default)]
    reveal_battalion_equivalents: Option<i32>,
    #[serde(default)]
    reveal_all: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ReconRaw {
    modifier: ReconModifierRaw,
    row: Vec<ReconRowRaw>,
}

/// The Air Recon of Land Units table (`airlog:42.27`).
#[derive(Debug, Clone)]
pub struct ReconLandUnits {
    per_planes: i32,
    add: i32,
    rows: Vec<(IntRange, i32)>,
    all_from: i32,
}

impl Bound for ReconLandUnits {
    const ID: &'static str = "airlog.42.27.air_recon_land_units";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: ReconRaw = raw.deserialize()?;
        if r.modifier.per_planes <= 0 {
            return Err(raw.err("modifier.per_planes", "must be positive"));
        }
        let mut rows = Vec::new();
        let mut all_from = None;
        for (i, row) in r.row.iter().enumerate() {
            match (
                row.modified_roll,
                row.modified_roll_min,
                row.reveal_battalion_equivalents,
                row.reveal_all,
            ) {
                (Some(range), None, Some(n), false) if n >= 0 => rows.push((range, n)),
                (None, Some(min), None, true) if all_from.is_none() && i + 1 == r.row.len() => {
                    all_from = Some(min);
                }
                _ => {
                    return Err(raw.err(
                        format!("row[{i}]"),
                        "needs modified_roll with a battalion count, or (last row only) modified_roll_min with reveal_all",
                    ));
                }
            }
        }
        let all_from =
            all_from.ok_or_else(|| raw.err("row", "missing the open-ended `reveal all` row"))?;
        let first = rows.first().map_or(all_from, |(r, _)| r.lo);
        check_int_tiling(rows.iter().map(|(r, _)| *r), first, all_from - 1)
            .map_err(|m| raw.err("row.modified_roll", m))?;
        if first != 1 {
            return Err(raw.err("row", "the first row must start at a modified roll of 1"));
        }
        Ok(Self {
            per_planes: r.modifier.per_planes,
            add: r.modifier.add,
            rows,
            all_from,
        })
    }
}

impl ReconLandUnits {
    /// What a reconnaissance of a land-units hex reveals, from the die roll and the number of
    /// planes that completed the mission (one is added to the roll for every four).
    /// `airlog:42.27`.
    pub fn reveal(&self, die: Die, planes_completed: i32) -> Reveal {
        let modified = i32::from(die.value()) + planes_completed / self.per_planes * self.add;
        if modified >= self.all_from {
            return Reveal::All;
        }
        self.rows
            .iter()
            .find(|(r, _)| r.contains(modified))
            .map_or(Reveal::BattalionEquivalents(0), |(_, n)| {
                Reveal::BattalionEquivalents(*n)
            })
    }
}

// ---------------------------------------------------------------------------------------------
// 42.53 Air Recon of Axis Naval Convoys
// ---------------------------------------------------------------------------------------------

/// Convoy size classes the Axis may report (`airlog:42.53`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvoySizeClass {
    Small,
    Medium,
    Large,
}

/// A size class and its tonnage bounds (the bounds overlap on purpose).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SizeClassBounds {
    pub class: ConvoySizeClass,
    #[serde(default)]
    pub min_tons: Option<i32>,
    #[serde(default)]
    pub max_tons: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
struct ConvoyReconRowRaw {
    #[serde(default)]
    planes: Option<i32>,
    #[serde(default)]
    planes_min: Option<i32>,
    respond_on_roll_at_most: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct ConvoyReconRaw {
    size_classes: Vec<SizeClassBounds>,
    row: Vec<ConvoyReconRowRaw>,
}

/// The Air Recon of Axis Naval Convoys table (`airlog:42.53`).
#[derive(Debug, Clone)]
pub struct ReconAxisConvoys {
    size_classes: Vec<SizeClassBounds>,
    /// Thresholds for 1, 2, ... planes; the last applies to its plane count and above.
    thresholds: Vec<i32>,
}

impl Bound for ReconAxisConvoys {
    const ID: &'static str = "airlog.42.53.air_recon_axis_convoys";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: ConvoyReconRaw = raw.deserialize()?;
        if r.row.is_empty() {
            return Err(raw.err("row", "needs at least one plane-count row"));
        }
        for class in [
            ConvoySizeClass::Small,
            ConvoySizeClass::Medium,
            ConvoySizeClass::Large,
        ] {
            if r.size_classes.iter().filter(|c| c.class == class).count() != 1 {
                return Err(raw.err(
                    "size_classes",
                    format!("{class:?} must appear exactly once"),
                ));
            }
        }
        let mut thresholds: Vec<i32> = Vec::new();
        for (i, row) in r.row.iter().enumerate() {
            if !is_reading(row.respond_on_roll_at_most) {
                return Err(raw.err(
                    format!("row[{i}].respond_on_roll_at_most"),
                    "must be a real two-dice reading (11-66)",
                ));
            }
            let expected = thresholds.len() as i32 + 1;
            let last = i + 1 == r.row.len();
            match (row.planes, row.planes_min) {
                (Some(p), None) if p == expected && !last => {}
                (None, Some(p)) if p == expected && last => {}
                _ => {
                    return Err(raw.err(
                        format!("row[{i}]"),
                        format!("expected plane count {expected} (last row: planes_min)"),
                    ));
                }
            }
            if thresholds
                .last()
                .is_some_and(|&t| row.respond_on_roll_at_most < t)
            {
                return Err(raw.err(
                    format!("row[{i}].respond_on_roll_at_most"),
                    "more planes must not lower the threshold",
                ));
            }
            thresholds.push(row.respond_on_roll_at_most);
        }
        Ok(Self {
            size_classes: r.size_classes,
            thresholds,
        })
    }
}

impl ReconAxisConvoys {
    /// Highest reading at which the Axis must answer for a lane scouted by `planes` planes; the
    /// last row applies to that many planes or more. `None` for no planes. `airlog:42.53`.
    pub fn respond_on_roll_at_most(&self, planes: i32) -> Option<i32> {
        if planes < 1 {
            return None;
        }
        let i = (planes as usize - 1).min(self.thresholds.len() - 1);
        Some(self.thresholds[i])
    }

    /// Whether the Axis must answer on this reading. `airlog:42.53`.
    pub fn must_respond(&self, planes: i32, reading: TwoDiceReading) -> bool {
        self.respond_on_roll_at_most(planes)
            .is_some_and(|t| i32::from(reading.value()) <= t)
    }

    /// The size classes and their (overlapping) tonnage bounds. `airlog:42.53`.
    pub fn size_classes(&self) -> &[SizeClassBounds] {
        &self.size_classes
    }
}

// ---------------------------------------------------------------------------------------------
// 44.41 Axis Strategic Airforce Commitment
// ---------------------------------------------------------------------------------------------

/// The scenarios of the commitment chart (`airlog:44.41`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaltaScenario {
    CampaignGame,
    GrazianiOffensive,
    RaceForTobruk,
    Crusader,
    LastChance,
    LongRetreat,
}

/// How long an availability level may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaltaCommitment {
    /// A cap in Game-Turns.
    GameTurns(i32),
    Unlimited,
    /// The level is not allowed in this scenario.
    NotApplicable,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum CommitmentRaw {
    Turns(i32),
    Word(String),
}

#[derive(Debug, Clone, Deserialize)]
struct CommitmentRowRaw {
    scenario: MaltaScenario,
    #[serde(default)]
    level_1: Option<CommitmentRaw>,
    #[serde(default)]
    level_2: Option<CommitmentRaw>,
    #[serde(default)]
    level_3: Option<CommitmentRaw>,
    #[serde(default)]
    level_4: Option<CommitmentRaw>,
    #[serde(default)]
    not_applicable: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CommitmentRaw2 {
    row: Vec<CommitmentRowRaw>,
}

/// The Axis Strategic Airforce Commitment chart (`airlog:44.41`).
#[derive(Debug, Clone)]
pub struct MaltaCommitmentTable {
    rows: Vec<(MaltaScenario, [MaltaCommitment; 4])>,
}

impl Bound for MaltaCommitmentTable {
    const ID: &'static str = "airlog.44.41.axis_strategic_airforce_commitment";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: CommitmentRaw2 = raw.deserialize()?;
        let mut rows = Vec::new();
        for (i, row) in r.row.iter().enumerate() {
            let cells = [&row.level_1, &row.level_2, &row.level_3, &row.level_4];
            let mut out = [MaltaCommitment::NotApplicable; 4];
            for (l, cell) in cells.iter().enumerate() {
                let level = l + 1;
                let na = row
                    .not_applicable
                    .iter()
                    .any(|n| *n == format!("level_{level}"));
                out[l] = match (cell, na) {
                    (Some(CommitmentRaw::Turns(n)), false) if *n > 0 => {
                        MaltaCommitment::GameTurns(*n)
                    }
                    (Some(CommitmentRaw::Word(w)), false) if w == "unlimited" => {
                        MaltaCommitment::Unlimited
                    }
                    (None, true) => MaltaCommitment::NotApplicable,
                    _ => {
                        return Err(raw.err(
                            format!("row[{i}].level_{level}"),
                            "needs exactly one of a positive turn count, \"unlimited\", or a not_applicable entry",
                        ));
                    }
                };
            }
            rows.push((row.scenario, out));
        }
        for s in [
            MaltaScenario::CampaignGame,
            MaltaScenario::GrazianiOffensive,
            MaltaScenario::RaceForTobruk,
            MaltaScenario::Crusader,
            MaltaScenario::LastChance,
            MaltaScenario::LongRetreat,
        ] {
            if rows.iter().filter(|(x, _)| *x == s).count() != 1 {
                return Err(raw.err("row", format!("{s:?} must appear exactly once")));
            }
        }
        Ok(Self { rows })
    }
}

impl MaltaCommitmentTable {
    /// How many Game-Turns of strategic bombardment of Malta an availability level (1-4) may be
    /// used in a scenario. `None` for a level outside 1-4. `airlog:44.41`.
    pub fn commitment(&self, scenario: MaltaScenario, level: usize) -> Option<MaltaCommitment> {
        let (_, cells) = self.rows.iter().find(|(s, _)| *s == scenario)?;
        cells.get(level.checked_sub(1)?).copied()
    }
}

// ---------------------------------------------------------------------------------------------
// 44.42 Axis Malta Availability
// ---------------------------------------------------------------------------------------------

/// Forces available for the bombardment of Malta, as percentages of the planes on
/// Sicily and Italy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct MaltaForces {
    /// Percent of each in-play plane type that may take part.
    pub in_play_percent: i32,
    /// Percent used to work out the strategic forces not in play (fractions rounded down).
    pub strategic_percent: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ForcesRaw {
    Forces(MaltaForces),
    Word(String),
}

#[derive(Debug, Clone, Deserialize)]
struct MaltaAvailRowRaw {
    dice_total: i32,
    level_1: ForcesRaw,
    level_2: ForcesRaw,
    level_3: ForcesRaw,
    level_4: ForcesRaw,
}

#[derive(Debug, Clone, Deserialize)]
struct MaltaAvailRaw {
    row: Vec<MaltaAvailRowRaw>,
}

/// The Axis Malta Availability Table (`airlog:44.42`).
#[derive(Debug, Clone)]
pub struct MaltaAvailability {
    /// Rows for dice totals 2-12, each with the four availability levels.
    rows: Vec<(i32, [Option<MaltaForces>; 4])>,
}

impl Bound for MaltaAvailability {
    const ID: &'static str = "airlog.44.42.axis_malta_availability";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: MaltaAvailRaw = raw.deserialize()?;
        check_int_tiling(
            r.row
                .iter()
                .map(|x| IntRange::new(x.dice_total, x.dice_total)),
            2,
            12,
        )
        .map_err(|m| raw.err("row.dice_total", m))?;
        let mut rows = Vec::new();
        for (i, row) in r.row.iter().enumerate() {
            let mut cells = [None; 4];
            for (l, cell) in [&row.level_1, &row.level_2, &row.level_3, &row.level_4]
                .into_iter()
                .enumerate()
            {
                cells[l] = match cell {
                    ForcesRaw::Forces(f) if f.in_play_percent > 0 && f.strategic_percent > 0 => {
                        Some(*f)
                    }
                    ForcesRaw::Word(w) if w == "na" => None,
                    _ => {
                        return Err(raw.err(
                            format!("row[{i}].level_{}", l + 1),
                            "needs positive percentages or \"na\"",
                        ));
                    }
                };
            }
            rows.push((row.dice_total, cells));
        }
        Ok(Self { rows })
    }
}

impl MaltaAvailability {
    /// Forces available on a two-dice total (2-12) at an availability level (1-4); `None` when
    /// the chart prints `na` (no forces this Game-Turn) or an input is out of range.
    /// `airlog:44.42`.
    pub fn forces(&self, level: usize, dice_total: i32) -> Option<MaltaForces> {
        let (_, cells) = self.rows.iter().find(|(t, _)| *t == dice_total)?;
        *cells.get(level.checked_sub(1)?)?
    }
}

// ---------------------------------------------------------------------------------------------
// 44.5 Maltese Air Facility Construction
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct ConstructionRowRaw {
    die: u8,
    levels: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct ConstructionRaw {
    row: Vec<ConstructionRowRaw>,
}

/// The Maltese Air Facility Construction table (`airlog:44.5`).
#[derive(Debug, Clone)]
pub struct MalteseConstruction {
    levels: [i32; 6],
}

impl Bound for MalteseConstruction {
    const ID: &'static str = "airlog.44.5.maltese_air_facility_construction";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: ConstructionRaw = raw.deserialize()?;
        let mut levels = [0; 6];
        let mut seen = [false; 6];
        for (i, row) in r.row.iter().enumerate() {
            if !(1..=6).contains(&row.die) || row.levels < 0 {
                return Err(raw.err(format!("row[{i}]"), "die must be 1-6 and levels >= 0"));
            }
            let slot = usize::from(row.die - 1);
            if std::mem::replace(&mut seen[slot], true) {
                return Err(raw.err(format!("row[{i}].die"), "die face given twice"));
            }
            levels[slot] = row.levels;
        }
        if seen.iter().any(|s| !s) {
            return Err(raw.err("row", "every die face 1-6 is needed"));
        }
        Ok(Self { levels })
    }
}

impl MalteseConstruction {
    /// Levels of air facility repaired and/or built by one die roll. `airlog:44.5`.
    pub fn levels(&self, die: Die) -> i32 {
        self.levels[usize::from(die.value() - 1)]
    }
}

// ---------------------------------------------------------------------------------------------
// 45.4 Maneuver Adjustment
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct ManeuverRowRaw {
    #[serde(default)]
    maneuver_differential: Option<IntRange>,
    #[serde(default)]
    maneuver_differential_min: Option<i32>,
    adjustment: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct ManeuverRaw {
    row: Vec<ManeuverRowRaw>,
}

/// The Air-Air Combat Maneuver Adjustment chart (`airlog:45.4`).
#[derive(Debug, Clone)]
pub struct ManeuverAdjustment {
    rows: Vec<(IntRange, i32)>,
    open_from: i32,
    open_adjustment: i32,
}

impl Bound for ManeuverAdjustment {
    const ID: &'static str = "airlog.45.4.maneuver_adjustment";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: ManeuverRaw = raw.deserialize()?;
        let mut rows = Vec::new();
        let mut open = None;
        for (i, row) in r.row.iter().enumerate() {
            match (row.maneuver_differential, row.maneuver_differential_min) {
                (Some(range), None) => rows.push((range, row.adjustment)),
                (None, Some(min)) if open.is_none() && i + 1 == r.row.len() => {
                    open = Some((min, row.adjustment));
                }
                _ => {
                    return Err(raw.err(
                        format!("row[{i}]"),
                        "needs maneuver_differential, or (last row only) maneuver_differential_min",
                    ));
                }
            }
        }
        let (open_from, open_adjustment) =
            open.ok_or_else(|| raw.err("row", "missing the open-ended last row"))?;
        check_int_tiling(rows.iter().map(|(r, _)| *r), 0, open_from - 1)
            .map_err(|m| raw.err("row.maneuver_differential", m))?;
        if rows.windows(2).any(|w| w[1].1 < w[0].1)
            || rows.last().is_some_and(|l| l.1 > open_adjustment)
        {
            return Err(raw.err(
                "row.adjustment",
                "the adjustment must not fall as the gap widens",
            ));
        }
        Ok(Self {
            rows,
            open_from,
            open_adjustment,
        })
    }
}

impl ManeuverAdjustment {
    /// Adjustment to the TacAir differential for a gap between the two planes Maneuver Ratings
    /// (the absolute difference). It improves the more maneuverable plane and worsens the
    /// other by the same amount. `airlog:45.4`.
    pub fn adjustment(&self, maneuver_gap: i32) -> i32 {
        let gap = maneuver_gap.unsigned_abs().min(i32::MAX as u32) as i32;
        if gap >= self.open_from {
            return self.open_adjustment;
        }
        self.rows
            .iter()
            .find(|(r, _)| r.contains(gap))
            .map(|(_, a)| *a)
            .expect("validated: the rows tile 0 up to the open-ended row")
    }
}

// ---------------------------------------------------------------------------------------------
// 45.5 TacAir Kill
// ---------------------------------------------------------------------------------------------

/// The roll needed to eliminate the defending plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillThreshold {
    /// The plane can never be eliminated at this differential.
    Never,
    /// Eliminated on a two-dice reading (11-66) equal to or less than this.
    AtMost(u8),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum KillRaw {
    Reading(i32),
    Word(String),
}

#[derive(Debug, Clone, Deserialize)]
struct KillRowRaw {
    #[serde(default)]
    differential: Option<IntRange>,
    #[serde(default)]
    differential_min: Option<i32>,
    #[serde(default)]
    differential_max: Option<i32>,
    kill_on_roll_at_most: KillRaw,
}

#[derive(Debug, Clone, Deserialize)]
struct KillRaw2 {
    row: Vec<KillRowRaw>,
}

/// The TacAir Kill Table (`airlog:45.5`).
#[derive(Debug, Clone)]
pub struct TacAirKill {
    /// Differentials at or below this use `below`.
    low_max: i32,
    below: KillThreshold,
    /// Middle rows, ascending and contiguous.
    rows: Vec<(IntRange, KillThreshold)>,
    /// Differentials at or above this use `above`.
    high_min: i32,
    above: KillThreshold,
}

impl Bound for TacAirKill {
    const ID: &'static str = "airlog.45.5.tacair_kill";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: KillRaw2 = raw.deserialize()?;
        let mut low = None;
        let mut high = None;
        let mut rows = Vec::new();
        for (i, row) in r.row.iter().enumerate() {
            let t = match &row.kill_on_roll_at_most {
                KillRaw::Reading(v) if is_reading(*v) => KillThreshold::AtMost(*v as u8),
                KillRaw::Word(w) if w == "na" => KillThreshold::Never,
                _ => {
                    return Err(raw.err(
                        format!("row[{i}].kill_on_roll_at_most"),
                        "must be a real two-dice reading (11-66) or \"na\"",
                    ));
                }
            };
            match (row.differential, row.differential_min, row.differential_max) {
                (Some(d), None, None) => rows.push((d, t)),
                (None, None, Some(max)) if low.is_none() && i == 0 => low = Some((max, t)),
                (None, Some(min), None) if high.is_none() && i + 1 == r.row.len() => {
                    high = Some((min, t));
                }
                _ => {
                    return Err(raw.err(
                        format!("row[{i}]"),
                        "needs differential, or differential_max (first row) / differential_min (last row)",
                    ));
                }
            }
        }
        let (low_max, below) =
            low.ok_or_else(|| raw.err("row", "missing the open-ended first row"))?;
        let (high_min, above) =
            high.ok_or_else(|| raw.err("row", "missing the open-ended last row"))?;
        check_int_tiling(rows.iter().map(|(r, _)| *r), low_max + 1, high_min - 1)
            .map_err(|m| raw.err("row.differential", m))?;
        let mut last = 0;
        for t in std::iter::once(&below)
            .chain(rows.iter().map(|(_, t)| t))
            .chain(std::iter::once(&above))
        {
            let value = match t {
                KillThreshold::Never => 0,
                KillThreshold::AtMost(v) => *v,
            };
            if value < last {
                return Err(raw.err(
                    "row.kill_on_roll_at_most",
                    "thresholds must not fall as the differential increases",
                ));
            }
            last = value;
        }
        Ok(Self {
            low_max,
            below,
            rows,
            high_min,
            above,
        })
    }
}

impl TacAirKill {
    /// The kill threshold at an adjusted TacAir differential (the firing plane's total TacAir
    /// plus the maneuver adjustment, minus the opponent total). `interp:airlog-0005`: the table
    /// governs over the worked example that quotes +2 differently. `airlog:45.5`.
    pub fn threshold(&self, differential: i32) -> KillThreshold {
        if differential <= self.low_max {
            return self.below;
        }
        if differential >= self.high_min {
            return self.above;
        }
        self.rows
            .iter()
            .find(|(r, _)| r.contains(differential))
            .map(|(_, t)| *t)
            .expect("validated: the rows tile the middle range")
    }

    /// Whether a two-dice reading eliminates the defending plane. `airlog:45.5`.
    pub fn kills(&self, differential: i32, reading: TwoDiceReading) -> bool {
        match self.threshold(differential) {
            KillThreshold::Never => false,
            KillThreshold::AtMost(t) => reading.value() <= t,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 45.6 Pilot and Plane Recovery
// ---------------------------------------------------------------------------------------------

/// How a plane was lost (`airlog:45.6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossCause {
    /// Destroyed by a result on the Strafing table.
    Strafed,
    /// A fighter or fighter-bomber lost in plane-to-plane combat or to flak.
    FlakAirAir,
}

/// What happens to a lost plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaneFate {
    Repairable,
    Lost,
}

/// What happens to the pilot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PilotFate {
    Survives,
    Killed,
}

/// The outcome of one recovery die roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recovery {
    pub plane: PlaneFate,
    /// `None` for strafed planes: the chart gives no pilot result for them.
    pub pilot: Option<PilotFate>,
}

#[derive(Debug, Clone, Deserialize)]
struct RecoveryRowRaw {
    cause: LossCause,
    die_range: IntRange,
    plane: PlaneFate,
    #[serde(default)]
    pilot: Option<PilotFate>,
}

#[derive(Debug, Clone, Deserialize)]
struct RecoveryRaw {
    row: Vec<RecoveryRowRaw>,
}

/// The Pilot and Plane Recovery Table (`airlog:45.6`).
#[derive(Debug, Clone)]
pub struct PilotAndPlaneRecovery {
    rows: Vec<RecoveryRowRaw>,
}

impl Bound for PilotAndPlaneRecovery {
    const ID: &'static str = "airlog.45.6.plane_and_pilot_recovery";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: RecoveryRaw = raw.deserialize()?;
        for cause in [LossCause::Strafed, LossCause::FlakAirAir] {
            check_int_tiling(
                r.row
                    .iter()
                    .filter(|x| x.cause == cause)
                    .map(|x| x.die_range),
                1,
                6,
            )
            .map_err(|m| raw.err(format!("row ({cause:?})"), m))?;
        }
        Ok(Self { rows: r.row })
    }
}

impl PilotAndPlaneRecovery {
    /// The fate of a plane (and pilot) lost for `cause`, by one die. `airlog:45.6`.
    pub fn recover(&self, cause: LossCause, die: Die) -> Recovery {
        let v = i32::from(die.value());
        let row = self
            .rows
            .iter()
            .find(|x| x.cause == cause && x.die_range.contains(v))
            .expect("validated: the rows tile 1-6");
        Recovery {
            plane: row.plane,
            pilot: row.pilot,
        }
    }
}
