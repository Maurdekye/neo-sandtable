//! One field recce/AC roll for an admitted land:22.14 group at a hex.
use super::*;
use crate::land::breakdown::{Asset, markers::BrokenMarker};

/// Ephemeral grouping only; source nationality stays on its original unit.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Group {
    NativeCommonwealth,
    Nationality { original: String, captured: bool },
}

fn class<'a>(
    c: &'a CnaContent,
    asset: &Asset,
) -> Result<&'a cna_content::units::UnitClass, Rejection> {
    let class = crate::land::formation::class(c, &asset.unit)
        .ok_or_else(|| gap("land:22.24", "recce source class is unresolved"))?;
    if asset.equipment != Equipment::ArmoredRecce || class.unit_type != "recce" {
        return Err(gap(
            "land:22.24",
            "selected body has no verified recce/AC class",
        ));
    }
    Ok(class)
}

/// Actor ownership does not overwrite original nationality or captured grouping.
/// Cases: land:22.11, land:22.14, land:22.24
pub(super) fn group(c: &CnaContent, marker: &BrokenMarker) -> Result<Group, Rejection> {
    let asset = &marker.assets[0];
    class(c, asset)?;
    let source = c
        .units
        .units
        .get(&asset.unit)
        .ok_or_else(|| gap("land:22.14", "recce original nationality is unresolved"))?;
    let captured = source.side != marker.side;
    if source.nationality.is_empty()
        || (!captured
            && marker.side == Side::Axis
            && !matches!(source.nationality.as_str(), "italian" | "german"))
    {
        return Err(gap(
            "land:22.14",
            "this original nationality group is not implemented",
        ));
    }
    if !captured && marker.side == Side::Commonwealth {
        Ok(Group::NativeCommonwealth)
    } else {
        Ok(Group::Nationality {
            original: source.nationality.clone(),
            captured,
        })
    }
}

/// Unknown composition and ancillary histories are never discarded to create receipts.
/// Cases: land:22.11, land:22.14, land:22.24, land:22.5
pub(super) fn validate(c: &CnaContent, s: &State, marker: &BrokenMarker) -> Result<(), Rejection> {
    class(c, &marker.assets[0])?;
    group(c, marker)?;
    if marker.tank_fuel.get() < 0 || marker.activity_water.get() < 0 {
        return Err(invariant("negative recce marker reserve").into());
    }
    let site = logistics::cargo_history::CargoSite::BrokenMarker(marker.id.clone());
    if marker.source_pool.is_some()
        || marker.assets[0].cohort.is_some()
        || marker.tank_fuel.get() != 0
        || marker.activity_water.get() != 0
        || !marker.passengers.is_empty()
        || marker.transport != Default::default()
        || !marker.fuel_cohorts.is_empty()
        || !marker.pool_fuel_cohorts.is_empty()
        || marker.paid_truck_water != Default::default()
        || marker.cargo.totals().map_err(supply)? != Supplies::default()
        || s.logistics
            .cargo_history
            .motion
            .entries
            .iter()
            .any(|entry| entry.site == site && !entry.cohorts.is_empty())
    {
        return Err(gap(
            "land:22.5",
            "recce ancillary or cohort partitions require their exact physical adapter",
        ));
    }
    Ok(())
}

/// Owner list order selects which point receives the sole possible success.
/// All rows were derived from the accepted plan before this one group die.
/// Cases: land:22.14, land:22.24, land:22.5
/// Interpretations: interp:land-0038, interp:land-0052
pub(super) fn resolve(
    c: &CnaContent,
    s: &mut State,
    cx: &mut Cx<'_>,
    rows: &[&RepairAttempt],
) -> Result<(), Rejection> {
    let first = rows
        .first()
        .ok_or_else(|| invariant("empty recce repair group"))?;
    let die = cx.rng.d6();
    let Some(BrokenRepairResult::ArmoredCarReccePoints(points)) = c
        .tables
        .land
        .broken_down_vehicle_repair
        .field_result(RepairVehicle::Recce, i32::from(die.value()))
    else {
        return Err(invariant("bound recce field chart result is unavailable").into());
    };
    if points != i32::from(die.value() == 1) {
        return Err(invariant("recce field chart must return one point only on a one").into());
    }
    if points == 0 {
        cx.emit(EngineEvent::new(Audience::Side(s.cursor.phasing(s.turn.player_a)
            .ok_or_else(|| invariant("recce actor disappeared"))?), GameEvent::Note {
                text: format!("Own recce/AC field group at {:?}: die {}, no points repaired; no supplies spent.", first.site(), die.value()) }));
        return Ok(());
    }
    let before = s.clone();
    let marker = s
        .land
        .breakdown
        .markers
        .get(first.marker())
        .cloned()
        .ok_or_else(|| invariant("recce marker disappeared"))?;
    let RepairAsset::Unit(selected) = first.asset() else {
        return Err(invariant("unexpected pool recce body").into());
    };
    let equipment = class(c, selected)?.id.clone();
    let mut remainder = marker.clone();
    remainder.assets[0].points -= 1;
    let after = (remainder.assets[0].points > 0).then_some(remainder);
    if let Some(after) = &after {
        s.land
            .breakdown
            .markers
            .insert(marker.id.clone(), after.clone());
    } else {
        s.land.breakdown.markers.remove(&marker.id);
    }
    let stage = WaterStage::current(s);
    let transition = ValidatedRepairOrTowOutcome {
        actor: marker.side,
        stage,
        before,
        after: s.clone(),
        cargo_moves: vec![],
        truck_moves: vec![],
    };
    let relocation = logistics::prepare_recovery_relocation(s, &transition)?;
    logistics::apply_recovery_relocation(s, &relocation)?;
    let mut success = selected.clone();
    success.points = 1;
    let output = ValidatedRepairOutcome {
        actor: marker.side,
        stage,
        site: first.site().clone(),
        markers: vec![MarkerRepairOutcome {
            before: marker.clone(),
            after,
            repaired_unit_assets: vec![success],
            repaired_pool_assets: vec![],
            junk_unit_assets: vec![],
            junk_pool_assets: vec![],
            receipts: vec![ReceiptReturn {
                equipment: ReturnedEquipment::RecceAc { equipment },
                points: 1,
            }],
            returned_trucks: None,
        }],
    };
    let ids = receipts::insert_replacement_receipts(c, s, &output)?;
    cx.emit(EngineEvent::new(Audience::Side(marker.side), GameEvent::Note {
        text: format!("Own recce/AC field group at {:?}: die {}, one selected point from {} returned as repair receipt {:?}; no supplies spent.", first.site(), die.value(), marker.id, ids) }));
    Ok(())
}
