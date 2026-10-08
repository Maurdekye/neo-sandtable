//! Printed fleet arrivals; scenario activation and port choices belong to procedures.
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ShipType {
    #[serde(rename = "BB")]
    Battleship,
    #[serde(rename = "CA")]
    HeavyCruiser,
    #[serde(rename = "CL")]
    LightCruiser,
    #[serde(rename = "CLAA")]
    AntiAircraftCruiser,
    #[serde(rename = "DD")]
    Destroyer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaltaGroup {
    Star,
    Dagger,
}
impl MaltaGroup {
    /// Maximum ships from this marked initial group that may deploy at Malta.
    /// Cases: land:30.6
    pub fn selection_limit(self) -> usize {
        match self {
            Self::Star => 1,
            Self::Dagger => 2,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetShip {
    #[serde(rename = "type")]
    pub ship_type: ShipType,
    /// Native chart spelling; this is a designation, not an allocated runtime identity.
    pub name: String,
    #[serde(default)]
    pub footnotes: Vec<MaltaGroup>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    when: String,
    op_stage: u8,
    game_turn: u16,
    ships: Vec<FleetShip>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Footnote {
    id: MaltaGroup,
    text: String,
}
#[derive(Deserialize)]
struct Body {
    group: Vec<Group>,
    footnote: Vec<Footnote>,
}
#[derive(Debug, Clone)]
pub struct CommonwealthFleetReinforcement {
    groups: BTreeMap<(u16, u8), Vec<FleetShip>>,
}
impl Bound for CommonwealthFleetReinforcement {
    const ID: &'static str = "land.30.6.cw_fleet_reinforcement";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: Body = raw.deserialize()?;
        let mut notes = BTreeSet::new();
        for (i, note) in body.footnote.into_iter().enumerate() {
            if note.text.trim().is_empty() || !notes.insert(note.id) {
                return Err(raw.err(format!("footnote[{i}]"), "nonempty unique note required"));
            }
        }
        if notes != BTreeSet::from([MaltaGroup::Star, MaltaGroup::Dagger]) {
            return Err(raw.err("footnote", "both Malta allocation notes are required"));
        }
        let mut groups = BTreeMap::new();
        let mut names = BTreeSet::new();
        let mut marked = BTreeMap::<MaltaGroup, usize>::new();
        for (i, group) in body.group.into_iter().enumerate() {
            let key = (group.game_turn, group.op_stage);
            let (expected, ship_count) = match group.when.as_str() {
                "deploy" => ((1, 1), 11),
                "3/8" => ((8, 3), 5),
                "1/33" => ((33, 1), 3),
                _ => return Err(raw.err(format!("group[{i}].when"), "unknown chart arrival")),
            };
            if key != expected || group.ships.len() != ship_count || groups.contains_key(&key) {
                return Err(raw.err(
                    format!("group[{i}]"),
                    "unique matching arrival and complete ship count required",
                ));
            }
            for (j, ship) in group.ships.iter().enumerate() {
                let field = format!("group[{i}].ships[{j}]");
                if ship.name.trim().is_empty() || !names.insert(ship.name.clone()) {
                    return Err(raw.err(format!("{field}.name"), "nonempty unique ship required"));
                }
                if ship.footnotes.len() > 1 || (!ship.footnotes.is_empty() && key != (1, 1)) {
                    return Err(raw.err(
                        format!("{field}.footnotes"),
                        "one initial-deployment mark at most",
                    ));
                }
                if let Some(mark) = ship.footnotes.first() {
                    if (*mark == MaltaGroup::Dagger && ship.ship_type != ShipType::Destroyer)
                        || (*mark == MaltaGroup::Star
                            && !matches!(
                                ship.ship_type,
                                ShipType::LightCruiser | ShipType::AntiAircraftCruiser
                            ))
                    {
                        return Err(
                            raw.err(format!("{field}.footnotes"), "mark and ship type disagree")
                        );
                    }
                    *marked.entry(*mark).or_default() += 1;
                }
            }
            groups.insert(key, group.ships);
        }
        if groups.keys().copied().collect::<BTreeSet<_>>()
            != BTreeSet::from([(1, 1), (8, 3), (33, 1)])
        {
            return Err(raw.err("group", "all three printed arrivals are required"));
        }
        if marked != BTreeMap::from([(MaltaGroup::Star, 3), (MaltaGroup::Dagger, 7)]) {
            return Err(raw.err(
                "group.ships.footnotes",
                "three star and seven dagger ships required",
            ));
        }
        Ok(Self { groups })
    }
}
impl CommonwealthFleetReinforcement {
    /// Exact scheduled ships; other dates return None. Default deployment is Alexandria,
    /// with the two marked initial groups permitting the printed Malta alternatives.
    /// Cases: land:30.6
    pub fn ships(&self, game_turn: u16, op_stage: u8) -> Option<&[FleetShip]> {
        self.groups.get(&(game_turn, op_stage)).map(Vec::as_slice)
    }
}
