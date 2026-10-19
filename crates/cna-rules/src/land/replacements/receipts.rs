//! Oob repair receipts and approved site-return helpers.
//! Repair receipts are site accounting, separate from scheduled production bodies.
//! Land owns successful outcomes, resolved-once closure and marker changes; Airlog
//! owns history relocation. All calls belong to their one whole-State finish draft.
//! Cases: land:20.2, land:20.4, land:22.0, land:22.5
//! Interpretations: interp:land-0038
use crate::{
    CnaContent, State,
    land::recovery::{
        PreparedSiteTruckReturn, ReturnPoolTarget, ReturnedEquipment, ValidatedRepairOutcome,
    },
    logistics::{FuelTruckKind, SupplyError, capacity, water::WaterStage},
    state::Location,
};
use cna_content::{
    scenario::{Placement, Supplies},
    units::Trucks,
};
use cna_core::{engine::EngineError, quantity::WaterPoints};
use cna_protocol::Side;
use cna_tables::airlog::{supply::SupplyType, trucks::TruckType};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ReplacementReceiptState {
    pub(crate) next_serial: BTreeMap<Side, u64>,
    pub(crate) lots: BTreeMap<String, ReplacementReceiptLot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReplacementReceiptLot {
    pub(crate) id: String,
    pub(crate) owner: Side,
    pub(crate) site: Location,
    pub(crate) stage: WaterStage,
    pub(crate) equipment: ReplacementEquipment,
    pub(crate) points: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ReplacementEquipment {
    Tank { equipment: String },
    RecceAc { equipment: String },
    Gun { equipment: String },
}

fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("repair returns: {detail}"),
    }
}

fn source_gap(case: &str, detail: &str) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    }
}

#[allow(
    dead_code,
    reason = "Truck returns will validate the exact prepared State"
)]
fn same_state(a: &State, b: &State) -> Result<bool, EngineError> {
    // Exact ephemeral footprint only. Never log either snapshot or its JSON.
    let a = serde_json::to_value(a).map_err(|_| invariant("state footprint cannot serialize"))?;
    let b = serde_json::to_value(b).map_err(|_| invariant("state footprint cannot serialize"))?;
    Ok(a == b)
}

fn site(content: &CnaContent, location: &Location) -> Result<(), EngineError> {
    let Location::Hex { hex } = location else {
        return Err(invariant("repair output is not at its actual marker hex"));
    };
    if content.map.canonical(hex) != Some(hex) || content.map.get(hex).is_none() {
        return Err(source_gap(
            "land:22.31",
            "repair site coordinate is unresolved",
        ));
    }
    Ok(())
}

fn equipment(content: &CnaContent, kind: &ReplacementEquipment) -> Result<(), EngineError> {
    match kind {
        ReplacementEquipment::Tank { equipment } | ReplacementEquipment::Gun { equipment } => {
            let weapon = content.units.weapons.get(equipment).ok_or_else(|| {
                source_gap(
                    "land:22.5",
                    "repaired weapon has no verified equipment record",
                )
            })?;
            let expected = match kind {
                ReplacementEquipment::Tank { .. } => weapon.kind == "tank",
                _ => matches!(
                    weapon.kind.as_str(),
                    "gun" | "sp_gun" | "anti_tank" | "anti_air_light" | "anti_air_heavy"
                ),
            };
            if !expected {
                return Err(invariant("receipt class contradicts verified weapon kind"));
            }
        }
        ReplacementEquipment::RecceAc { equipment } => {
            let class = content.units.classes.get(equipment).ok_or_else(|| {
                source_gap("land:22.5", "repaired recce has no verified class identity")
            })?;
            if class.unit_type != "recce" {
                return Err(invariant(
                    "receipt recce identity is not a verified recce class",
                ));
            }
        }
    }
    Ok(())
}

fn prefix(owner: Side) -> String {
    format!("{}.repair-receipt-", crate::state::side_key(owner))
}

fn repaired_equipment(
    content: &CnaContent,
    asset: &crate::land::breakdown::Asset,
) -> Result<Option<ReplacementEquipment>, EngineError> {
    use crate::land::breakdown::Equipment;
    let kind = match &asset.equipment {
        Equipment::Weapon(id) => {
            let weapon = content.units.weapons.get(id).ok_or_else(|| {
                source_gap(
                    "land:22.5",
                    "successful weapon has no verified equipment record",
                )
            })?;
            if weapon.kind == "tank" {
                ReplacementEquipment::Tank {
                    equipment: id.clone(),
                }
            } else {
                ReplacementEquipment::Gun {
                    equipment: id.clone(),
                }
            }
        }
        Equipment::ArmoredRecce => {
            let class = crate::land::formation::class(content, &asset.unit)
                .ok_or_else(|| source_gap("land:22.5", "successful recce class is unresolved"))?;
            ReplacementEquipment::RecceAc {
                equipment: class.id.clone(),
            }
        }
        _ => return Ok(None),
    };
    equipment(content, &kind)?;
    Ok(Some(kind))
}

fn count_kind(
    totals: &mut BTreeMap<ReplacementEquipment, i32>,
    kind: ReplacementEquipment,
    points: i32,
) -> Result<(), EngineError> {
    if points <= 0 {
        return Err(invariant("successful equipment points must be positive"));
    }
    let total = totals
        .get(&kind)
        .copied()
        .unwrap_or(0)
        .checked_add(points)
        .ok_or_else(|| invariant("successful equipment total overflow"))?;
    totals.insert(kind, total);
    Ok(())
}

fn validate_lots(
    content: &CnaContent,
    receipts: &ReplacementReceiptState,
) -> Result<(), EngineError> {
    for (key, lot) in &receipts.lots {
        let serial = lot
            .id
            .strip_prefix(&prefix(lot.owner))
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| invariant("receipt identity is malformed"))?;
        if key != &lot.id
            || serial == 0
            || lot.id != format!("{}{serial}", prefix(lot.owner))
            || serial > receipts.next_serial.get(&lot.owner).copied().unwrap_or(0)
            || lot.points <= 0
            || lot.stage.game_turn == 0
            || !(1..=3).contains(&lot.stage.op_stage)
        {
            return Err(invariant("receipt key/count/stage/serial is corrupt"));
        }
        site(content, &lot.site)?;
        equipment(content, &lot.equipment)?;
    }
    Ok(())
}

fn add_lots(
    content: &CnaContent,
    receipts: &mut ReplacementReceiptState,
    owner: Side,
    location: &Location,
    stage: WaterStage,
    rows: &[(ReplacementEquipment, i32)],
) -> Result<Vec<String>, EngineError> {
    validate_lots(content, receipts)?;
    site(content, location)?;
    if stage.game_turn == 0 || !(1..=3).contains(&stage.op_stage) {
        return Err(invariant("repair stage is invalid"));
    }
    let mut draft = receipts.clone();
    let mut ids = vec![];
    for (kind, points) in rows {
        equipment(content, kind)?;
        if *points <= 0 {
            return Err(invariant("successful receipt points must be positive"));
        }
        let next = draft
            .next_serial
            .get(&owner)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| invariant("receipt identity space exhausted"))?;
        let id = format!("{}{next}", prefix(owner));
        if draft.lots.contains_key(&id) {
            return Err(invariant("receipt identity already used"));
        }
        draft.next_serial.insert(owner, next);
        draft.lots.insert(
            id.clone(),
            ReplacementReceiptLot {
                id: id.clone(),
                owner,
                site: location.clone(),
                stage,
                equipment: kind.clone(),
                points: *points,
            },
        );
        ids.push(id);
    }
    *receipts = draft;
    Ok(ids)
}

/// Land's trusted outcome is the successful-count authority, never an actor input.
/// This helper has no repeat ledger: Land's resolved-once finish supplies exactly-once.
/// Cases: land:20.2, land:20.4, land:22.0, land:22.5
pub(crate) fn insert_replacement_receipts(
    content: &CnaContent,
    state: &mut State,
    outcome: &ValidatedRepairOutcome,
) -> Result<Vec<String>, EngineError> {
    site(content, outcome.site())?;
    if outcome.stage() != WaterStage::current(state) {
        return Err(invariant("repair stage changed"));
    }
    let mut seen = BTreeSet::new();
    let mut rows = vec![];
    for marker in outcome.markers() {
        if marker.before().side != outcome.actor()
            || outcome.site()
                != &(Location::Hex {
                    hex: marker.before().hex.clone(),
                })
            || !seen.insert(&marker.before().id)
        {
            return Err(invariant("repair receipt marker/site/owner mismatch"));
        }
        let mut successful = BTreeMap::new();
        let mut remainder = marker.before().assets.clone();
        for asset in marker.repaired_unit_assets() {
            let source = remainder
                .iter_mut()
                .find(|old| {
                    old.unit == asset.unit
                        && old.equipment == asset.equipment
                        && old.cohort == asset.cohort
                })
                .ok_or_else(|| invariant("successful equipment is absent from source marker"))?;
            if asset.points <= 0 || asset.points > source.points {
                return Err(invariant(
                    "successful equipment exceeds source marker quantity",
                ));
            }
            source.points -= asset.points;
            if let Some(kind) = repaired_equipment(content, asset)? {
                count_kind(&mut successful, kind, asset.points)?;
            }
        }
        if marker.repaired_pool_assets().iter().any(|asset| {
            !matches!(
                asset.equipment,
                crate::land::breakdown::Equipment::LightTruck
                    | crate::land::breakdown::Equipment::MediumTruck
                    | crate::land::breakdown::Equipment::HeavyTruck
            )
        }) {
            return Err(invariant(
                "real pool repair outcome contains nontruck equipment",
            ));
        }
        let mut recorded = BTreeMap::new();
        for receipt in marker.receipts() {
            let kind = match receipt.equipment() {
                ReturnedEquipment::Tank { equipment } => ReplacementEquipment::Tank {
                    equipment: equipment.clone(),
                },
                ReturnedEquipment::RecceAc { equipment } => ReplacementEquipment::RecceAc {
                    equipment: equipment.clone(),
                },
                ReturnedEquipment::Gun { equipment } => ReplacementEquipment::Gun {
                    equipment: equipment.clone(),
                },
            };
            count_kind(&mut recorded, kind.clone(), receipt.points())?;
            rows.push((kind, receipt.points()));
        }
        if recorded != successful {
            return Err(invariant(
                "receipt totals disagree with successful equipment",
            ));
        }
    }
    add_lots(
        content,
        &mut state.land.replacement_receipts,
        outcome.actor(),
        outcome.site(),
        outcome.stage(),
        &rows,
    )
}

/// Called only after the owned allocation procedure admits its actor/site/type plan.
/// No attachment, training, recipient body, capacity or movement is granted here.
/// Cases: land:20.2, land:20.4, land:22.5
#[allow(
    dead_code,
    reason = "Receipt delivery awaits its replacement allocation procedure"
)]
pub(crate) fn consume_replacement_receipt(
    content: &CnaContent,
    state: &mut State,
    owner: Side,
    id: &str,
    points: i32,
) -> Result<(), EngineError> {
    let receipts = &mut state.land.replacement_receipts;
    validate_lots(content, receipts)?;
    let lot = receipts
        .lots
        .get(id)
        .ok_or_else(|| invariant("receipt absent or exhausted"))?;
    if lot.owner != owner || points <= 0 || points > lot.points {
        return Err(invariant("receipt consumption owner/count mismatch"));
    }
    let remaining = lot
        .points
        .checked_sub(points)
        .ok_or_else(|| invariant("receipt subtraction overflow"))?;
    if remaining == 0 {
        receipts.lots.remove(id);
    } else {
        receipts
            .lots
            .get_mut(id)
            .ok_or_else(|| invariant("receipt vanished"))?
            .points = remaining;
    }
    Ok(())
}

#[allow(
    dead_code,
    reason = "Checked site-return arithmetic awaits truck repair"
)]
fn add(a: i32, b: i32) -> Result<i32, EngineError> {
    if a < 0 || b < 0 {
        return Err(invariant("negative returned holdings"));
    }
    a.checked_add(b)
        .ok_or_else(|| invariant("returned holdings overflow"))
}

#[allow(
    dead_code,
    reason = "Checked site-return arithmetic awaits truck repair"
)]
fn add_trucks(a: Trucks, b: Trucks) -> Result<Trucks, EngineError> {
    Ok(Trucks {
        light: add(a.light, b.light)?,
        medium: add(a.medium, b.medium)?,
        heavy: add(a.heavy, b.heavy)?,
    })
}

#[allow(
    dead_code,
    reason = "Checked site-return arithmetic awaits truck repair"
)]
fn add_cargo(a: Supplies, b: Supplies) -> Result<Supplies, EngineError> {
    Ok(Supplies {
        ammo: add(a.ammo, b.ammo)?,
        fuel: add(a.fuel, b.fuel)?,
        stores: add(a.stores, b.stores)?,
        water: add(a.water, b.water)?,
    })
}

#[allow(
    dead_code,
    reason = "Truck-return capacity validation awaits truck repair"
)]
fn within(a: Supplies, b: Supplies) -> bool {
    a.ammo <= b.ammo && a.fuel <= b.fuel && a.stores <= b.stores && a.water <= b.water
}

#[allow(
    dead_code,
    reason = "Truck-return capacity validation awaits truck repair"
)]
fn merged_capacity(
    content: &CnaContent,
    trucks: Trucks,
    cargo: Supplies,
) -> Result<(), EngineError> {
    // No pool passengers or new per-type packing storage. Derive an exact valid
    // packing transiently from the complete merged aggregate, then discard it.
    for kind in [TruckType::Light, TruckType::Medium, TruckType::Heavy] {
        for goods in [
            SupplyType::Ammo,
            SupplyType::Fuel,
            SupplyType::Stores,
            SupplyType::Water,
        ] {
            if content
                .tables
                .airlog
                .truck_characteristics
                .truck(kind)
                .supply_capacity(goods)
                <= 0
            {
                return Err(source_gap(
                    "airlog:54.2",
                    "returned-pool cargo capacity unavailable",
                ));
            }
        }
    }
    let packing = capacity::find_packing(content, &trucks, &Trucks::default(), cargo)
        .ok_or_else(|| invariant("prepared return has no valid complete merged cargo packing"))?;
    capacity::validate_packing(content, &trucks, &Trucks::default(), &cargo, &packing).map_err(
        |error| match error {
            SupplyError::Unsupported { case } => {
                source_gap(case, "returned-pool cargo capacity unavailable")
            }
            _ => invariant("prepared return exceeds complete merged pool capacity"),
        },
    )
}

/// Real truck counts/cargo/water only. Neither marker nor history/funding writers
/// are called. Land and Airlog perform their own approved changes on the same draft.
/// Cases: land:22.0, land:22.5, airlog:53.11, airlog:54.2
/// Interpretations: interp:land-0038
#[allow(
    dead_code,
    reason = "Approved truck-return applier awaits the truck repair producer"
)]
pub(crate) fn apply_site_truck_return(
    content: &CnaContent,
    state: &mut State,
    prepared: &PreparedSiteTruckReturn,
) -> Result<Option<String>, EngineError> {
    if !same_state(state, prepared.before())? || prepared.stage() != WaterStage::current(state) {
        return Err(invariant("site return footprint or stage changed"));
    }
    site(content, prepared.site())?;
    let mut trucks = Trucks::default();
    let mut cargo = Supplies::default();
    let mut water = 0;
    let mut markers = BTreeSet::new();
    let mut cohorts = BTreeSet::new();
    for returned in prepared.returns() {
        let marker = state
            .land
            .breakdown
            .markers
            .get(returned.marker())
            .ok_or_else(|| invariant("returned-truck marker disappeared"))?;
        if !markers.insert(returned.marker())
            || marker.side != prepared.actor()
            || prepared.site()
                != &(Location::Hex {
                    hex: marker.hex.clone(),
                })
        {
            return Err(invariant("site-return marker/owner/site mismatch"));
        }
        let same_target = match (prepared.target(), returned.target()) {
            (ReturnPoolTarget::CreateAtSite, ReturnPoolTarget::CreateAtSite) => true,
            (ReturnPoolTarget::Existing { pool: a }, ReturnPoolTarget::Existing { pool: b }) => {
                a == b
            }
            _ => false,
        };
        if !same_target {
            return Err(invariant("return target differs from pre-dice target"));
        }
        let counts = add_trucks(Trucks::default(), returned.trucks())?;
        if counts == Trucks::default() {
            return Err(invariant("zero successful batch must be omitted"));
        }
        let mut available = Trucks::default();
        for (equipment, points) in marker
            .assets
            .iter()
            .map(|a| (&a.equipment, a.points))
            .chain(marker.pool_assets.iter().map(|a| (&a.equipment, a.points)))
        {
            use crate::land::breakdown::Equipment;
            match equipment {
                Equipment::LightTruck => available.light = add(available.light, points)?,
                Equipment::MediumTruck => available.medium = add(available.medium, points)?,
                Equipment::HeavyTruck => available.heavy = add(available.heavy, points)?,
                _ => {}
            }
        }
        if counts.light > available.light
            || counts.medium > available.medium
            || counts.heavy > available.heavy
        {
            return Err(invariant("successful trucks exceed marker body"));
        }
        let mut known = Trucks::default();
        for cohort in returned.physical_cohorts() {
            if cohort.id.is_empty()
                || cohort.count <= 0
                || cohort.parent.as_ref() == Some(&cohort.id)
                || !cohorts.insert(&cohort.id)
            {
                return Err(invariant("invalid returned physical cohort"));
            }
            match cohort.kind {
                FuelTruckKind::Light => known.light = add(known.light, cohort.count)?,
                FuelTruckKind::Medium => known.medium = add(known.medium, cohort.count)?,
                FuelTruckKind::Heavy => known.heavy = add(known.heavy, cohort.count)?,
            }
        }
        // Empty legacy evidence is not seeded into known zero motion. Airlog
        // independently validates exact selected/remainder/relocated histories.
        if !returned.physical_cohorts().is_empty() && known != counts {
            return Err(invariant("returned physical cohorts disagree with counts"));
        }
        let goods = add_cargo(Supplies::default(), returned.cargo())?;
        let held = marker
            .cargo
            .totals()
            .map_err(|_| invariant("marker cargo is malformed"))?;
        if !within(goods, held) {
            return Err(invariant("returned cargo exceeds marker cargo"));
        }
        let mut named = Supplies::default();
        let mut lots = BTreeSet::new();
        for parcel in returned.cargo_parcels() {
            if parcel.lot.is_empty()
                || !lots.insert(&parcel.lot)
                || parcel.spent_cp_quarters < 0
                || parcel.ceiling_cp_quarters.is_some_and(|n| n < 0)
            {
                return Err(invariant("invalid returned cargo parcel"));
            }
            named = add_cargo(named, parcel.goods)?;
        }
        if !within(named, goods) {
            return Err(invariant("named cargo exceeds actual returned goods"));
        }
        let activity = add(0, returned.activity_water().get())?;
        if activity > marker.activity_water.get() {
            return Err(invariant("returned water exceeds actual reserve"));
        }
        trucks = add_trucks(trucks, counts)?;
        cargo = add_cargo(cargo, goods)?;
        water = add(water, activity)?;
    }
    if trucks == Trucks::default() {
        if cargo != Supplies::default() || water != 0 {
            return Err(invariant("zero return carries holdings"));
        }
        return Ok(None);
    }
    let mut draft = state.logistics.clone();
    let id = match prepared.target() {
        ReturnPoolTarget::Existing { pool } => {
            let mut matches = draft.truck_pools.iter_mut().filter(|p| p.id == *pool);
            let target = matches
                .next()
                .ok_or_else(|| invariant("existing return pool missing"))?;
            if matches.next().is_some()
                || target.side != prepared.actor()
                || target.location.as_ref() != Some(prepared.site())
            {
                return Err(invariant(
                    "existing return pool owner/site/identity mismatch",
                ));
            }
            let merged_trucks = add_trucks(target.trucks, trucks)?;
            let merged_cargo = add_cargo(target.cargo, cargo)?;
            let merged_water = add(target.activity_water.get(), water)?;
            merged_capacity(content, merged_trucks, merged_cargo)?;
            target.trucks = merged_trucks;
            target.cargo = merged_cargo;
            target.activity_water = WaterPoints::new(merged_water);
            // All preexisting tank fuel/box state/history/accounts remain intact.
            pool.clone()
        }
        ReturnPoolTarget::CreateAtSite => {
            merged_capacity(content, trucks, cargo)?;
            let Location::Hex { hex } = prepared.site() else {
                return Err(invariant("return site changed"));
            };
            let id = crate::logistics::pools::add_truck_pool(
                &mut draft,
                None,
                prepared.actor(),
                Placement::Hex { hex: hex.clone() },
                Some(prepared.site().clone()),
                trucks,
                cargo,
            )
            .map_err(|_| invariant("site pool allocation failed"))?;
            draft
                .truck_pools
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or_else(|| invariant("allocated pool missing"))?
                .activity_water = WaterPoints::new(water);
            // Allocator defaults are new pool construction only: no tank fuel,
            // current funding binding, credit or fresh motion record is created.
            id
        }
    };
    state.logistics = draft;
    Ok(Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (CnaContent, State, Location, ReplacementEquipment) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let s = State::new(&c).unwrap();
        let location = Location::Hex {
            hex: c.map.iter().next().unwrap().id.clone(),
        };
        let id = c
            .units
            .weapons
            .values()
            .find(|weapon| weapon.kind == "tank")
            .unwrap()
            .id
            .clone();
        (c, s, location, ReplacementEquipment::Tank { equipment: id })
    }

    /// Cases: land:20.2, land:20.4, land:22.5
    #[test]
    fn receipt_partial_exhaustion_keeps_side_serial_and_never_reuses_id() {
        let (c, mut s, location, kind) = fixture();
        let stage = WaterStage::current(&s);
        let ids = add_lots(
            &c,
            &mut s.land.replacement_receipts,
            Side::Axis,
            &location,
            stage,
            &[(kind.clone(), 3)],
        )
        .unwrap();
        consume_replacement_receipt(&c, &mut s, Side::Axis, &ids[0], 1).unwrap();
        assert_eq!(s.land.replacement_receipts.lots[&ids[0]].points, 2);
        consume_replacement_receipt(&c, &mut s, Side::Axis, &ids[0], 2).unwrap();
        assert!(!s.land.replacement_receipts.lots.contains_key(&ids[0]));
        let before = serde_json::to_vec(&s).unwrap();
        assert!(matches!(
            consume_replacement_receipt(&c, &mut s, Side::Axis, &ids[0], 1),
            Err(EngineError::Invariant { .. })
        ));
        assert_eq!(serde_json::to_vec(&s).unwrap(), before);
        let next = add_lots(
            &c,
            &mut s.land.replacement_receipts,
            Side::Axis,
            &location,
            stage,
            &[(kind, 1)],
        )
        .unwrap();
        assert_ne!(next[0], ids[0]);
        assert_eq!(s.land.replacement_receipts.next_serial[&Side::Axis], 2);
    }

    /// Cases: land:20.2, land:20.4, land:22.5, land:3.6
    #[test]
    fn foreign_nonpositive_and_excess_consumption_preserve_complete_state() {
        let (c, mut s, location, kind) = fixture();
        let stage = WaterStage::current(&s);
        let id = add_lots(
            &c,
            &mut s.land.replacement_receipts,
            Side::Axis,
            &location,
            stage,
            &[(kind, 2)],
        )
        .unwrap()
        .remove(0);
        for (side, points) in [
            (Side::Commonwealth, 1),
            (Side::Axis, 0),
            (Side::Axis, -1),
            (Side::Axis, 3),
        ] {
            let before = serde_json::to_vec(&s).unwrap();
            assert!(matches!(
                consume_replacement_receipt(&c, &mut s, side, &id, points),
                Err(EngineError::Invariant { .. })
            ));
            assert_eq!(serde_json::to_vec(&s).unwrap(), before);
        }
    }

    /// Cases: land:20.2, land:22.5
    #[test]
    fn late_serial_overflow_and_unknown_equipment_rollback_all_new_lots() {
        let (c, mut s, location, kind) = fixture();
        let stage = WaterStage::current(&s);
        s.land
            .replacement_receipts
            .next_serial
            .insert(Side::Axis, u64::MAX - 1);
        let before = serde_json::to_vec(&s.land.replacement_receipts).unwrap();
        assert!(matches!(
            add_lots(
                &c,
                &mut s.land.replacement_receipts,
                Side::Axis,
                &location,
                stage,
                &[(kind.clone(), 1), (kind.clone(), 1)]
            ),
            Err(EngineError::Invariant { .. })
        ));
        assert_eq!(
            serde_json::to_vec(&s.land.replacement_receipts).unwrap(),
            before
        );
        s.land.replacement_receipts.next_serial.clear();
        let before = serde_json::to_vec(&s.land.replacement_receipts).unwrap();
        let unknown = ReplacementEquipment::Tank {
            equipment: "unverified.test-equipment".into(),
        };
        assert!(
            matches!(add_lots(&c,&mut s.land.replacement_receipts,Side::Axis,&location,stage,&[(kind,1),(unknown,1)]),Err(EngineError::Unsupported { case,.. }) if case == "land:22.5")
        );
        assert_eq!(
            serde_json::to_vec(&s.land.replacement_receipts).unwrap(),
            before
        );
    }

    /// Cases: airlog:53.11, airlog:54.2, land:22.5
    #[test]
    fn capacity_uses_whole_merged_goods_and_checked_truck_totals() {
        let (c, _, _, _) = fixture();
        assert!(
            merged_capacity(
                &c,
                Trucks {
                    heavy: 1,
                    ..Trucks::default()
                },
                Supplies::default()
            )
            .is_ok()
        );
        assert!(matches!(
            merged_capacity(
                &c,
                Trucks::default(),
                Supplies {
                    ammo: 1,
                    ..Supplies::default()
                }
            ),
            Err(EngineError::Invariant { .. })
        ));
        assert!(matches!(
            add_trucks(
                Trucks {
                    light: i32::MAX,
                    ..Trucks::default()
                },
                Trucks {
                    light: 1,
                    ..Trucks::default()
                }
            ),
            Err(EngineError::Invariant { .. })
        ));
        assert!(matches!(
            add_cargo(
                Supplies {
                    water: i32::MAX,
                    ..Supplies::default()
                },
                Supplies {
                    water: 1,
                    ..Supplies::default()
                }
            ),
            Err(EngineError::Invariant { .. })
        ));
    }
}
