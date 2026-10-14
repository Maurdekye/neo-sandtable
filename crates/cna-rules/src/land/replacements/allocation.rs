//! Source-derived payment and atomic cohex absorption; no reassignment or upgrades.
use super::*;
use crate::{CnaContent, State, steps::illegal};
use cna_content::units::{InfantryKind, NormalToe, Toe, WeaponPoints};
use cna_core::{engine::Rejection, ids::SeatId};
use cna_tables::land::{
    capability::{CpAction, CpCost},
    replacements::{
        ReplacementCost, ReplacementPointKind as Point, ReplacementRequirement,
        ReplacementUnit as Row,
    },
};
use std::collections::BTreeMap;

fn source_row(content: &CnaContent, id: &UnitId) -> Result<Row, EngineError> {
    let source = content
        .units
        .units
        .get(id)
        .ok_or_else(|| invariant("allocation recipient source absent"))?;
    let class = crate::land::formation::class(content, id).ok_or_else(|| {
        gap(
            "land:20.3",
            "recipient has no source-bound conversion category",
        )
    })?;
    match class.unit_type.as_str() {
        "headquarters" => Ok(Row::AnyHeadquartersUnit),
        "tank" => Ok(Row::Tank),
        "artillery" => Ok(Row::Artillery),
        "anti_tank" => Ok(Row::AntiTank),
        "anti_air" => Ok(Row::AntiAir),
        "engineer" => match source.echelon.as_deref().or(class.echelon.as_deref()) {
            Some("battalion") => Ok(Row::EngineerBattalion),
            Some("company") => Ok(Row::EngineerCompany),
            _ => Err(gap(
                "land:20.3",
                "engineer conversion requires its verified echelon",
            )),
        },
        "infantry" => {
            if source.id == UnitId::new("cw.unassigned_inf.a_bn_special_service_bde") {
                return Ok(Row::Commando);
            }
            if matches!(class.id.as_str(), "cw.h" | "cw.j") {
                return Err(gap(
                    "land:20.3",
                    "special infantry requires an explicit conversion identity",
                ));
            }
            match source.infantry_kind {
                Some(InfantryKind::Ordinary) if source.side != Side::Commonwealth => Err(gap(
                    "land:20.3",
                    "Axis ordinary counter subtype does not distinguish Bersaglieri or airborne conversion identity",
                )),
                Some(InfantryKind::Ordinary) => Ok(Row::AnyOtherInfantry),
                Some(InfantryKind::MachineGun) => Ok(Row::Machinegun),
                Some(InfantryKind::HeavyWeapons) => Ok(Row::HeavyWeapons),
                None => Err(gap("land:20.3", "infantry conversion identity is unknown")),
            }
        }
        _ => Err(gap(
            "land:20.3",
            "recce/special conversion category is not inferred from a name or actor choice",
        )),
    }
}

fn point(content: &CnaContent, kind: &ReplacementKind) -> Result<Point, EngineError> {
    match kind {
        ReplacementKind::Infantry { .. } => Ok(Point::Infantry),
        ReplacementKind::Tank { weapon, .. } => {
            if !content
                .units
                .weapons
                .get(weapon)
                .is_some_and(|weapon| weapon.kind == "tank")
            {
                return Err(invariant(
                    "tank replacement has a non-tank physical identity",
                ));
            }
            Ok(Point::Tank)
        }
        ReplacementKind::Recce { .. } => Ok(Point::ArmoredRecon),
        ReplacementKind::Gun { weapon, .. } => {
            let equipment = content
                .units
                .weapons
                .get(weapon)
                .ok_or_else(|| invariant("replacement weapon absent"))?;
            if equipment.airdroppable {
                return Err(gap(
                    "land:20.3",
                    "airdroppable gun conversion needs its verified special point and recipient category",
                ));
            }
            match equipment.kind.as_str() {
                "artillery" => Ok(Point::ArtilleryGun),
                "anti_tank" => Ok(Point::AntiTankGun),
                "anti_air" => Ok(Point::AntiAirGun),
                _ => Err(gap(
                    "land:20.3",
                    "gun lacks a verified replacement classification",
                )),
            }
        }
    }
}

fn multiplier(cost: &ReplacementCost, paid: &BTreeMap<Point, i32>) -> Option<i32> {
    let mut required = cost.points().clone();
    let mut paid = paid.clone();
    if required.contains_key(&Point::AnyGun) {
        let total = [Point::ArtilleryGun, Point::AntiTankGun, Point::AntiAirGun]
            .into_iter()
            .map(|kind| i64::from(paid.remove(&kind).unwrap_or(0)))
            .sum::<i64>();
        paid.insert(Point::AnyGun, i32::try_from(total).ok()?);
    }
    required.retain(|_, count| *count != 0);
    paid.retain(|_, count| *count != 0);
    if required.len() != paid.len() {
        return None;
    }
    let mut units = None;
    for (kind, price) in required {
        let count = *paid.get(&kind)?;
        if price <= 0 || count <= 0 || count % price != 0 {
            return None;
        }
        let n = count / price;
        if units.is_some_and(|old| old != n) {
            return None;
        }
        units = Some(n);
    }
    units
}

fn toe_gain(content: &CnaContent, row: Row, paid: &BTreeMap<Point, i32>) -> Result<i32, Rejection> {
    let requirement = content.tables.land.replacement_conversion.requirement(row);
    let gain = match requirement {
        ReplacementRequirement::All(cost) => multiplier(cost, paid),
        ReplacementRequirement::Alternatives(costs) => {
            costs.iter().find_map(|cost| multiplier(cost, paid))
        }
        ReplacementRequirement::NoPoints => None,
    };
    gain.filter(|gain| *gain > 0)
        .ok_or_else(|| illegal("payment is not one complete source conversion option"))
}

fn cp(content: &CnaContent, action: CpAction) -> Result<i32, EngineError> {
    match content.tables.land.capability_expenditure.cost(action) {
        Some(CpCost::Flat(price)) => price
            .checked_mul(4)
            .ok_or_else(|| invariant("CP cost overflow")),
        _ => Err(gap(
            "land:6.3",
            "replacement CP cost has no verified flat price",
        )),
    }
}

fn charge_unit(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    quarters: i32,
) -> Result<(), Rejection> {
    let allowance = crate::land::formation::individual_allowance(content, state, id)
        .ok_or_else(|| Rejection::Engine(gap("land:20.48", "recipient/parent CPA is unknown")))?;
    let unit = state
        .land
        .units
        .get_mut(id)
        .ok_or_else(|| illegal("receiving unit vanished"))?;
    crate::land::capability::charge(unit, allowance, quarters, false)
}

/// One disposable draft validates all recipients/payments/costs before publication.
/// No enemy ZOC query: legal cohex absorption is allowed even in enemy ZOC.
/// Cases: land:20.3, land:20.41, land:20.42, land:20.48, land:20.49, land:6.3
pub(super) fn prepare(
    content: &CnaContent,
    state: &State,
    seat: SeatId,
    plan: &AllocationPlan,
) -> Result<State, Rejection> {
    if state.cursor.anchor() != "opstage.organization.reorganization" {
        return Err(illegal("absorption is restricted to Reorganization"));
    }
    if plan.uses.is_empty() {
        return Ok(state.clone());
    }
    let current = super::training::stage(state).map_err(Rejection::Engine)?;
    let mut groups: BTreeMap<UnitId, Vec<&AllocationUse>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for usage in &plan.uses {
        if !seen.insert(&usage.counter) {
            return Err(illegal("replacement counter paid twice"));
        }
        let counter = super::training::owned(content, state, seat, &usage.counter)?;
        if counter.physical.stage > current || counter.physical.cp_quarters < 0 {
            return Err(Rejection::Engine(invariant(
                "invalid replacement physical stage or CP history",
            )));
        }
        if counter.recipient != usage.recipient {
            return Err(illegal(
                "later reassignment is not initial assignment authority",
            ));
        }
        if !matches!(counter.training, Training::Ready) {
            return Err(illegal("untrained points cannot be absorbed"));
        }
        if counter.carriage.is_some() {
            return Err(Rejection::Engine(gap(
                "land:20.44",
                "carried-point unloading is a separate adapter",
            )));
        }
        let unit = &state.land.units[&usage.recipient];
        if !matches!(counter.location, Location::Hex { .. }) || counter.location != unit.location {
            return Err(illegal("trained points and recipient must share a map hex"));
        }
        if usage.points <= 0 || usage.points > counter.points {
            return Err(illegal("invalid replacement payment count"));
        }
        if usage.points == counter.points
            && (counter.physical.supplies != Supplies::default()
                || counter.physical.tank_fuel != FuelTenths::ZERO
                || counter.physical.water != WaterPoints::ZERO)
        {
            return Err(Rejection::Engine(gap(
                "land:20.45",
                "full absorption with held stock requires an exact stock-transfer adapter",
            )));
        }
        groups
            .entry(usage.recipient.clone())
            .or_default()
            .push(usage);
    }
    let mut draft = state.clone();
    for (recipient, uses) in groups {
        let row = source_row(content, &recipient).map_err(Rejection::Engine)?;
        let mut paid = BTreeMap::new();
        let mut weapons: BTreeMap<String, i32> = BTreeMap::new();
        for usage in &uses {
            let selected: Row =
                serde_json::from_value(serde_json::Value::String(usage.conversion.clone()))
                    .map_err(|_| illegal("unknown conversion row"))?;
            if selected != row {
                return Err(illegal("conversion is not the recipient's verified row"));
            }
            let counter = super::training::owned(content, state, seat, &usage.counter)?;
            let kind = point(content, &counter.kind).map_err(Rejection::Engine)?;
            let count = paid.entry(kind).or_insert(0_i32);
            *count = count
                .checked_add(usage.points)
                .ok_or_else(|| illegal("payment overflow"))?;
            if let ReplacementKind::Gun { weapon, .. } | ReplacementKind::Tank { weapon, .. } =
                &counter.kind
            {
                let count = weapons.entry(weapon.clone()).or_insert(0_i32);
                *count = count
                    .checked_add(usage.points)
                    .ok_or_else(|| illegal("equipment payment overflow"))?;
            }
        }
        let gain = toe_gain(content, row, &paid)?;
        let unit = &draft.land.units[&recipient];
        let source = &content.units.units[&recipient];
        let class = crate::land::formation::class(content, &recipient)
            .ok_or_else(|| Rejection::Engine(invariant("class vanished")))?;
        let mut maximum = class.max_toe.ok_or_else(|| {
            Rejection::Engine(gap("land:20.3", "recipient maximum TOE is unknown"))
        })?;
        if let Some(us_maximum) = class.max_toe_if_all_us_tanks {
            let existing_us = match &unit.toe {
                Some(Toe::Weapons(points)) => points.iter().all(|p| {
                    content
                        .units
                        .weapons
                        .get(&p.weapon)
                        .is_some_and(|w| w.us_tank)
                }),
                _ => source
                    .vehicle
                    .as_ref()
                    .and_then(|id| content.units.weapons.get(id))
                    .is_some_and(|w| w.us_tank),
            };
            if existing_us
                && weapons
                    .keys()
                    .all(|id| content.units.weapons.get(id).is_some_and(|w| w.us_tank))
            {
                maximum = us_maximum;
            }
        }
        let before = crate::logistics::toe_strength(content, unit)
            .map_err(|_| Rejection::Engine(gap("land:20.3", "recipient live TOE is unknown")))?
            .get();
        let after = before
            .checked_add(gain)
            .ok_or_else(|| illegal("TOE overflow"))?;
        if after > maximum {
            return Err(illegal("absorption exceeds the verified TOE maximum"));
        }
        let next_toe = if weapons.is_empty() {
            if matches!(unit.toe, Some(Toe::Weapons(_))) {
                return Err(illegal("infantry payment cannot rewrite an equipment TOE"));
            }
            let normal = class.max_toe.expect("maximum checked above");
            if after == normal {
                Toe::Normal(NormalToe::N)
            } else if after < normal {
                Toe::Under { under: after }
            } else {
                Toe::Over { over: after }
            }
        } else {
            let mut inventory = match &unit.toe {
                Some(Toe::Weapons(points)) => points
                    .iter()
                    .map(|p| (p.weapon.clone(), p.n))
                    .collect::<BTreeMap<_, _>>(),
                _ => {
                    let weapon = source.vehicle.clone().ok_or_else(|| {
                        Rejection::Engine(gap(
                            "land:20.3",
                            "existing equipment identity is unknown",
                        ))
                    })?;
                    BTreeMap::from([(weapon, before)])
                }
            };
            for (weapon, n) in weapons {
                let equipment = content
                    .units
                    .weapons
                    .get(&weapon)
                    .ok_or_else(|| Rejection::Engine(invariant("equipment vanished")))?;
                if equipment.kind != class.unit_type
                    && !class
                        .assigns
                        .iter()
                        .any(|a| a.kind == equipment.kind && a.max >= n)
                {
                    return Err(illegal("recipient cannot hold that equipment type"));
                }
                let count = inventory.entry(weapon).or_insert(0);
                *count = count
                    .checked_add(n)
                    .ok_or_else(|| illegal("weapon count overflow"))?;
            }
            if inventory.values().map(|n| i64::from(*n)).sum::<i64>() != i64::from(after) {
                return Err(illegal("equipment payment does not preserve derived TOE"));
            }
            for assignment in &class.assigns {
                let held = inventory
                    .iter()
                    .filter(|(id, _)| {
                        content
                            .units
                            .weapons
                            .get(*id)
                            .is_some_and(|weapon| weapon.kind == assignment.kind)
                    })
                    .map(|(_, n)| i64::from(*n))
                    .sum::<i64>();
                if held > i64::from(assignment.max) {
                    return Err(illegal(
                        "combined equipment exceeds the verified per-type capacity",
                    ));
                }
            }
            Toe::Weapons(
                inventory
                    .into_iter()
                    .filter(|(_, n)| *n > 0)
                    .map(|(weapon, n)| WeaponPoints { weapon, n })
                    .collect(),
            )
        };
        let attach = cp(content, CpAction::AttachAssignedUnit).map_err(Rejection::Engine)?;
        let absorb = cp(content, CpAction::AbsorbTwoToeReplacementPoints)
            .map_err(Rejection::Engine)?
            .checked_mul(gain)
            .ok_or_else(|| illegal("absorption CP overflow"))?
            / 2;
        let parent = crate::ownership::parent_for_unit(content, &draft, &recipient).cloned();
        if let Some(parent) = parent {
            charge_unit(content, &mut draft, &parent, absorb)?;
        }
        charge_unit(
            content,
            &mut draft,
            &recipient,
            attach
                .checked_mul(
                    i32::try_from(uses.len()).map_err(|_| illegal("attachment count overflow"))?,
                )
                .and_then(|cost| cost.checked_add(absorb))
                .ok_or_else(|| illegal("CP overflow"))?,
        )?;
        let cpa = crate::land::formation::individual_allowance(content, &draft, &recipient)
            .ok_or_else(|| {
                Rejection::Engine(gap("land:20.42", "assigned recipient CPA is unknown"))
            })?
            .cpa;
        for usage in uses {
            let counter = super::scheduling::side_mut(&mut draft, seat.side)
                .map_err(Rejection::Engine)?
                .counters
                .iter_mut()
                .find(|c| c.id == usage.counter)
                .ok_or_else(|| Rejection::Engine(invariant("counter vanished")))?;
            if counter.physical.stage < current {
                counter.physical.stage = current;
                counter.physical.cp_quarters = 0;
            }
            counter.physical.cp_quarters = counter
                .physical
                .cp_quarters
                .checked_add(attach)
                .ok_or_else(|| illegal("replacement CP overflow"))?;
            if i64::from(counter.physical.cp_quarters) > i64::from(cpa) * 4 {
                counter.physical.cohesion_quarters = -26 * 4;
            }
            counter.points -= usage.points;
        }
        draft
            .land
            .units
            .get_mut(&recipient)
            .ok_or_else(|| illegal("recipient vanished"))?
            .toe = Some(next_toe);
    }
    super::scheduling::side_mut(&mut draft, seat.side)
        .map_err(Rejection::Engine)?
        .counters
        .retain(|counter| counter.points != 0);
    Ok(draft)
}
