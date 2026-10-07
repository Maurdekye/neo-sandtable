//! Port ownership and one inbound/outbound supply budget per OpStage.
//! Accepted truth movement calls record_entry inside its transactional draft.
//! Air/construction mutate damage fields after checking their own cases.
use super::{SupplyError, water::WaterStage};
use crate::{
    CnaContent,
    state::{Location, State},
};
use cna_content::scenario::Supplies;
use cna_protocol::Side;
use cna_tables::airlog::{supply::SupplyType, trucks::PortName};
use serde::{Deserialize, Serialize};

/// Dynamic damage/control, not copied chart characteristics. Exact supply weights use
/// 1/24-ton units, avoiding rounding each fractional fuel/water shipment independently.
/// Cases: airlog:55.12, airlog:55.14, airlog:55.18, airlog:55.26, airlog:55.27, land:30.58
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortState {
    pub owner: Side,
    pub efficiency: i32,
    #[serde(default)]
    pub blocked_levels: i32,
    #[serde(default)]
    pub mined_levels: i32,
    #[serde(default)]
    pub bombed_stage: Option<WaterStage>,
    #[serde(default)]
    pub budget_stage: Option<WaterStage>,
    #[serde(default)]
    pub used_tons24: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub id: String,
    pub name: PortName,
    pub location: Location,
}
fn named(name: &str) -> PortName {
    match name.to_ascii_lowercase().as_str() {
        "tripoli" => PortName::Tripoli,
        "bizerta" | "tunis" => PortName::Bizerta,
        "alexandria" => PortName::Alexandria,
        "tobruk" => PortName::Tobruk,
        "benghazi" => PortName::Benghazi,
        "mersa matruh" | "mersa_matruh" => PortName::MersaMatruh,
        "bardia" => PortName::Bardia,
        "sollum" => PortName::Sollum,
        "derna" => PortName::Derna,
        _ => PortName::AllOthers,
    }
}
/// Resolve only a verified port icon or source-authorized off-map port. A city name alone
/// is not evidence of a port; missing map anchors remain Unsupported.
/// Cases: airlog:55.11, airlog:55.17, airlog:56.11, scen:59.54
pub fn at(content: &CnaContent, location: &Location) -> Result<Port, SupplyError> {
    match location {
        Location::OffMap { id } if id == "box_tripoli" => Ok(Port {
            id: id.clone(),
            name: PortName::Tripoli,
            location: location.clone(),
        }),
        Location::OffMap { id } if id == "box_tunis" => Ok(Port {
            id: id.clone(),
            name: PortName::Bizerta,
            location: location.clone(),
        }),
        Location::Hex { hex } => {
            let hex = content.map.canonical(hex).ok_or(SupplyError::Invalid)?;
            let p = content.places.at(hex).find(|p| p.kind == "port").ok_or(
                SupplyError::Unsupported {
                    case: "airlog:55.11",
                },
            )?;
            Ok(Port {
                id: hex.to_string(),
                name: named(&p.name),
                location: Location::Hex { hex: hex.clone() },
            })
        }
        _ => Err(SupplyError::Unsupported {
            case: "airlog:55.11",
        }),
    }
}
/// Resolve a convoy destination from the bound lane, without hard-coded hex coordinates.
/// Cases: airlog:56.11, airlog:56.12
pub fn lane_destination(content: &CnaContent, lane: u8) -> Result<Port, SupplyError> {
    let route = content
        .tables
        .airlog
        .convoy_air_distance
        .route(lane)
        .ok_or(SupplyError::Invalid)?;
    let name = route.rsplit("_to_").next().ok_or(SupplyError::Invalid)?;
    match name {
        "tripoli" => at(
            content,
            &Location::OffMap {
                id: "box_tripoli".into(),
            },
        ),
        "bizerta" => at(
            content,
            &Location::OffMap {
                id: "box_tunis".into(),
            },
        ),
        _ => {
            let p = content
                .places
                .places
                .values()
                .find(|p| p.kind == "port" && p.name.eq_ignore_ascii_case(name))
                .ok_or(SupplyError::Unsupported {
                    case: "airlog:56.11",
                })?;
            at(
                content,
                &Location::Hex {
                    hex: p.hex_id.clone(),
                },
            )
        }
    }
}
/// Mandatory scheduled reinforcements do not consume this budget; planned personnel
/// apply their own penalty before supplies. Existing state is not reinitialized.
/// Cases: airlog:55.15, airlog:56.24, scen:59.54
pub fn initialize(content: &CnaContent, state: &mut State) {
    if content.scenario.fleet_logistics.axis_coastal_shipping.as_ref().is_some_and(|f|matches!(&f.location,cna_content::scenario::Placement::City{city} if city=="tripoli")){
  let loc=Location::OffMap{id:"box_tripoli".into()};
  if !state.logistics.ports.contains_key("box_tripoli"){record_entry(content,state,Side::Axis,&loc);}
 }
    let entries: Vec<_> = state
        .land
        .units
        .values()
        .filter(|u| u.location.hex().is_some())
        .map(|u| (u.side, u.location.clone()))
        .collect();
    for (side, loc) in entries {
        if at(content, &loc).is_ok_and(|p| !state.logistics.ports.contains_key(&p.id)) {
            record_entry(content, state, side, &loc);
        }
    }
}
/// Record last accepted friendly entry only. A planning probe must never call this.
/// Existing damage and the shared throughput budget survive a change of owner.
/// Cases: airlog:55.11, airlog:55.14, land:30.58
pub fn record_entry(content: &CnaContent, state: &mut State, side: Side, location: &Location) {
    let Ok(port) = at(content, location) else {
        return;
    };
    let row = content.tables.airlog.port_capacity.port(port.name);
    let initial_block = if port.name == PortName::Tobruk { 3 } else { 0 };
    let stage = WaterStage::current(state);
    let entry = state
        .logistics
        .ports
        .entry(port.id)
        .or_insert_with(|| PortState {
            owner: side,
            efficiency: (row.max_efficiency_level - initial_block).max(0),
            blocked_levels: initial_block,
            mined_levels: 0,
            bombed_stage: None,
            budget_stage: Some(stage),
            used_tons24: 0,
        });
    entry.owner = side;
}
fn ordinal(stage: WaterStage) -> i32 {
    i32::from(stage.game_turn) * 3 + i32::from(stage.op_stage)
}
/// Quiet stages recover bombing damage, not blocking or unswept mines. Repeated calls
/// in one stage neither recover a second level nor reset the shared shipment budget.
/// Cases: airlog:55.18, airlog:55.26, airlog:55.27
pub fn advance(content: &CnaContent, state: &mut State, port: &Port) -> Result<(), SupplyError> {
    let stage = WaterStage::current(state);
    let row = content.tables.airlog.port_capacity.port(port.name);
    let p = state
        .logistics
        .ports
        .get_mut(&port.id)
        .ok_or(SupplyError::Unsupported {
            case: "airlog:55.11",
        })?;
    if p.efficiency < 0 || p.blocked_levels < 0 || p.mined_levels < 0 || p.used_tons24 < 0 {
        return Err(SupplyError::Invalid);
    }
    if p.budget_stage != Some(stage) {
        let quiet = p.budget_stage.map_or(0, |prior| {
            (ordinal(stage) - ordinal(prior) - i32::from(p.bombed_stage == Some(prior))).max(0)
        });
        let maximum = (row.max_efficiency_level - p.blocked_levels - p.mined_levels).max(0);
        p.efficiency = p.efficiency.saturating_add(quiet).min(maximum);
        p.budget_stage = Some(stage);
        p.used_tons24 = 0;
    }
    Ok(())
}
/// Exact weight of a mixed shipment before port/ship capacity comparison.
/// Cases: airlog:54.5
pub fn weight24(content: &CnaContent, cargo: &Supplies) -> Result<i64, SupplyError> {
    [
        SupplyType::Ammo,
        SupplyType::Fuel,
        SupplyType::Stores,
        SupplyType::Water,
    ]
    .into_iter()
    .try_fold(0i64, |sum, t| {
        let n = super::capacity::points(cargo, t);
        let r = content.tables.airlog.equivalent_weights.tons_per_point(t);
        if n < 0 || r.den <= 0 || 24 % r.den != 0 {
            return Err(SupplyError::Invalid);
        }
        sum.checked_add(
            i64::from(n)
                .checked_mul(i64::from(r.num))
                .and_then(|v| v.checked_mul(24 / i64::from(r.den)))
                .ok_or(SupplyError::Invalid)?,
        )
        .ok_or(SupplyError::Invalid)
    })
}
/// Largest supply tonnage at current efficiency, rounded upward as printed.
/// Cases: airlog:55.14, airlog:55.3
pub fn capacity_tons(
    content: &CnaContent,
    p: &PortState,
    name: PortName,
) -> Result<i64, SupplyError> {
    let row = content.tables.airlog.port_capacity.port(name);
    if p.efficiency < 0 || p.efficiency > row.max_efficiency_level {
        return Err(SupplyError::Invalid);
    }
    Ok(
        (i64::from(row.max_tonnage) * i64::from(p.efficiency)
            + i64::from(row.max_efficiency_level)
            - 1)
            / i64::from(row.max_efficiency_level),
    )
}
/// Arrival and coastal departure share this budget. Level G overflow at Tripoli cannot
/// bypass neutralization or enemy ownership. Bombing alone does not remove this exception.
/// Cases: airlog:55.14, airlog:55.17, airlog:55.3, airlog:56.27
/// Interpretations: interp:airlog-0010
pub fn charge(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    port: &Port,
    weight24: i64,
    level_g_tripoli: bool,
) -> Result<(), SupplyError> {
    if weight24 < 0 {
        return Err(SupplyError::Invalid);
    }
    let mut draft = state.clone();
    advance(content, &mut draft, port)?;
    if port.name == PortName::Bizerta && !draft.logistics.bizerta_open {
        return Err(SupplyError::Insufficient);
    }
    let p = draft.logistics.ports.get_mut(&port.id).unwrap();
    if p.owner != side || p.efficiency == 0 {
        return Err(SupplyError::Insufficient);
    }
    let total = p
        .used_tons24
        .checked_add(weight24)
        .ok_or(SupplyError::Invalid)?;
    let overflow = level_g_tripoli && port.name == PortName::Tripoli;
    if !overflow && total > capacity_tons(content, p, port.name)? * 24 {
        return Err(SupplyError::Insufficient);
    }
    p.used_tons24 = total;
    state.logistics = draft.logistics;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cna_core::ids::UnitId;
    fn setup() -> (CnaContent, State, Port) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        s.cursor.op_stage = Some(1);
        initialize(&c, &mut s);
        let p = at(
            &c,
            &Location::OffMap {
                id: "box_tripoli".into(),
            },
        )
        .unwrap();
        (c, s, p)
    }
    /// Cases: airlog:55.14, airlog:55.3
    /// Interpretations: interp:airlog-0010
    #[test]
    fn inbound_and_outbound_share_budget_and_reset_only_next_stage() {
        let (c, mut s, p) = setup();
        charge(&c, &mut s, Side::Axis, &p, 10000 * 24, false).unwrap();
        charge(&c, &mut s, Side::Axis, &p, 5000 * 24, false).unwrap();
        let before = serde_json::to_value(&s).unwrap();
        assert_eq!(
            charge(&c, &mut s, Side::Axis, &p, 1, false),
            Err(SupplyError::Insufficient)
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        s.cursor.op_stage = Some(2);
        charge(&c, &mut s, Side::Axis, &p, 15000 * 24, false).unwrap();
        assert_eq!(s.logistics.ports[&p.id].used_tons24, 15000 * 24);
    }
    /// Cases: airlog:54.5, airlog:55.14, airlog:55.18, airlog:55.26
    #[test]
    fn exact_mixed_weight_rounded_port_capacity_and_quiet_recovery() {
        let (c, mut s, p) = setup();
        assert_eq!(
            weight24(
                &c,
                &Supplies {
                    ammo: 1,
                    fuel: 1,
                    water: 1,
                    stores: 1
                }
            )
            .unwrap(),
            127
        );
        let example = PortState {
            owner: Side::Axis,
            efficiency: 1,
            blocked_levels: 0,
            mined_levels: 0,
            bombed_stage: None,
            budget_stage: None,
            used_tons24: 0,
        };
        assert_eq!(
            capacity_tons(&c, &example, PortName::Benghazi).unwrap(),
            834
        );
        let ps = s.logistics.ports.get_mut(&p.id).unwrap();
        ps.efficiency = 5;
        ps.blocked_levels = 2;
        ps.mined_levels = 1;
        for op in [2, 3] {
            s.cursor.op_stage = Some(op);
            advance(&c, &mut s, &p).unwrap();
            advance(&c, &mut s, &p).unwrap();
        }
        assert_eq!(s.logistics.ports[&p.id].efficiency, 7);
        s.cursor.game_turn = 2;
        s.cursor.op_stage = Some(1);
        advance(&c, &mut s, &p).unwrap();
        assert_eq!(s.logistics.ports[&p.id].efficiency, 7);
    }

    /// Cases: airlog:55.3
    #[test]
    fn level_g_tripoli_overflow_survives_bombing_but_not_neutralization() {
        let (c, mut s, p) = setup();
        s.logistics.ports.get_mut(&p.id).unwrap().efficiency = 1;
        charge(&c, &mut s, Side::Axis, &p, 40000 * 24, true).unwrap();
        s.logistics.ports.get_mut(&p.id).unwrap().efficiency = 0;
        assert!(charge(&c, &mut s, Side::Axis, &p, 24, true).is_err());
    }
    /// Cases: airlog:55.11, airlog:56.11, land:30.58
    #[test]
    fn last_entry_preserves_damage_and_unverified_city_is_not_port() {
        let (c, mut s, p) = setup();
        s.logistics.ports.get_mut(&p.id).unwrap().efficiency = 4;
        record_entry(&c, &mut s, Side::Commonwealth, &p.location);
        assert_eq!(s.logistics.ports[&p.id].owner, Side::Commonwealth);
        assert_eq!(s.logistics.ports[&p.id].efficiency, 4);
        assert!(charge(&c, &mut s, Side::Axis, &p, 24, false).is_err());
        let port = at(
            &c,
            &Location::Hex {
                hex: "C4022".into(),
            },
        )
        .unwrap();
        assert_eq!(port.name, PortName::Sollum);
        // Absence is an explicit fixture premise, independent of later surveys.
        let mut city_only = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        city_only
            .places
            .places
            .retain(|_, place| place.hex_id.as_str() != "A4827" || place.kind != "port");
        assert!(
            city_only
                .places
                .at(&"A4827".into())
                .any(|place| { place.kind == "major_city" && place.name == "Benghazi" })
        );
        assert_eq!(
            at(
                &city_only,
                &Location::Hex {
                    hex: "A4827".into()
                }
            ),
            Err(SupplyError::Unsupported {
                case: "airlog:55.11"
            })
        );
        assert_eq!(
            lane_destination(&city_only, 3),
            Err(SupplyError::Unsupported {
                case: "airlog:56.11"
            })
        );
        let id: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
        s.land.units.get_mut(&id).unwrap().location = port.location.clone();
        record_entry(&c, &mut s, Side::Axis, &port.location);
        assert_eq!(s.logistics.ports[&port.id].owner, Side::Axis);
    }
    /// Cases: airlog:55.11, airlog:56.11
    #[test]
    fn explicit_benghazi_port_registration_resolves_lane_three() {
        let (mut c, _, _) = setup();
        // A test-only explicit port registration, separate from the city record.
        // The resolver must use its kind; the city's name alone never suffices.
        let city = c.places.places["city-benghazi-a4827"].clone();
        c.places
            .places
            .retain(|_, place| place.hex_id.as_str() != "A4827" || place.kind != "port");
        assert!(lane_destination(&c, 3).is_err());
        let mut port_record = city;
        port_record.id = "fixture-port-benghazi-a4827".into();
        port_record.kind = "port".into();
        port_record.review_batch = "synthetic-port-resolver-fixture".into();
        port_record.note = Some("Synthetic test input, not a surveyed map record.".into());
        port_record.src.clear();
        c.places.places.insert(port_record.id.clone(), port_record);
        let port = at(
            &c,
            &Location::Hex {
                hex: "A4827".into(),
            },
        )
        .unwrap();
        assert_eq!(port.name, PortName::Benghazi);
        assert_eq!(
            port.location,
            Location::Hex {
                hex: "A4827".into()
            }
        );
        assert_eq!(lane_destination(&c, 3).unwrap(), port);
        assert!(
            c.places
                .at(&"A4827".into())
                .any(|place| { place.kind == "major_city" && place.name == "Benghazi" })
        );
        assert_eq!(
            at(
                &c,
                &Location::Hex {
                    hex: "C4022".into()
                }
            )
            .unwrap()
            .name,
            PortName::Sollum
        );
    }

    /// Cases: scen:60.37, airlog:56.11
    #[test]
    fn typed_fleet_flags_and_lanes_survive_reused_setup() {
        let (c, _, _) = setup();
        let fleet = c.scenario.fleet_logistics.axis_convoys.unwrap();
        assert_eq!(fleet.lanes_allowed, vec![2, 3, 6]);
        assert!(fleet.pre_game_plan_remaining_start_month);
        let reused = CnaContent::load(&cna_content::repo_data_dir(), "italian_campaign").unwrap();
        assert_eq!(
            reused
                .scenario
                .fleet_logistics
                .axis_convoys
                .unwrap()
                .lanes_allowed,
            vec![2, 3, 6]
        );
    }
}
