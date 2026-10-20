//! Same-site finite actual-SGSU gun and bomb service. No mission entitlement.
use super::*;
use crate::air::preparation::{CarriedOrdnance, KnownAmmunition, OrdnanceKnowledge};
use fueling::{ServicingChange, ValidatedFueling};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GunSelection {
    Preserve,
    Load,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BombLoad {
    pub mode: usize,
    pub bomb_points: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmingSelection {
    pub plane: PlaneId,
    pub guns: GunSelection,
    /// None preserves existing ordnance; it does not record an empty payload.
    pub bombs: Option<BombLoad>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmingCohort {
    pub servicer: SgsuId,
    pub planes: Vec<ArmingSelection>,
}

/// Exact new service IDs and the site where it happened, including gun loads
/// sharing a previously paid squadron charge. Never infer history from movement.
/// Cases: airlog:38.42, airlog:38.43
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmingReceipt {
    pub period: MaintenancePeriod,
    pub servicer: SgsuId,
    pub facility: FacilityId,
    pub planes: Vec<PlaneId>,
    pub gun_loads: Vec<(PlaneId, String)>,
}

/// One physical paid squadron charge, never a permanent ammunition entitlement.
/// Cases: airlog:38.43
/// Interpretations: interp:airlog-0024
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GunCharge {
    pub period: MaintenancePeriod,
    pub assigned_squadron: String,
    pub facility: FacilityId,
    pub payer: SgsuId,
    pub planes: Vec<PlaneId>,
}

#[derive(Clone)]
pub(super) struct ValidatedArming {
    pub(super) changes: BTreeMap<PlaneId, ServicingChange>,
    pub(super) demands: BTreeMap<SgsuId, SupplyDemand>,
    pub(super) active: BTreeMap<FacilityId, BTreeSet<SgsuId>>,
    pub(super) receipts: Vec<ArmingReceipt>,
    pub(super) charges: Vec<GunCharge>,
}

impl ValidatedArming {
    pub(super) fn add_demands(
        &self,
        demands: &mut BTreeMap<SgsuId, SupplyDemand>,
    ) -> Result<(), EngineError> {
        for (id, ammo) in &self.demands {
            let d = demands.entry(id.clone()).or_default();
            d.ammo = AmmoPoints::new(
                d.ammo
                    .get()
                    .checked_add(ammo.ammo.get())
                    .ok_or_else(|| invalid("arming ammunition demand overflow"))?,
            );
        }
        Ok(())
    }

    pub(super) fn union_active(&self, active: &mut BTreeMap<FacilityId, BTreeSet<SgsuId>>) {
        for (id, units) in &self.active {
            active
                .entry(id.clone())
                .or_default()
                .extend(units.iter().cloned());
        }
    }
}

/// No stock mutation, RNG or admission; only owner-known facts decide legality.
/// Cases: airlog:38.41, airlog:38.42, airlog:38.43, airlog:38.44, airlog:38.23
/// Interpretations: interp:airlog-0024
pub(super) fn validate(
    content: &CnaContent,
    state: &State,
    owner: Side,
    current: MaintenancePeriod,
    plan: &RefitPlan,
    fuel: &ValidatedFueling,
) -> Result<ValidatedArming, Rejection> {
    let mut result = ValidatedArming {
        changes: BTreeMap::new(),
        demands: BTreeMap::new(),
        active: BTreeMap::new(),
        receipts: Vec::new(),
        charges: Vec::new(),
    };
    let window = &state.air.runtime.maintenance;
    let prior = window
        .arming_receipts
        .get(&owner)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let charges = window
        .gun_charges
        .get(&owner)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let charged: BTreeMap<_, _> = charges
        .iter()
        .filter(|r| r.period == current)
        .map(|r| (r.assigned_squadron.clone(), r.facility.clone()))
        .collect();
    let mut seen: BTreeSet<_> = prior
        .iter()
        .filter(|r| r.period == current)
        .flat_map(|r| r.planes.iter().cloned())
        .collect();
    let mut counts = BTreeMap::<SgsuId, usize>::new();
    for receipt in prior.iter().filter(|r| r.period == current) {
        *counts.entry(receipt.servicer.clone()).or_default() += receipt.planes.len();
        result
            .active
            .entry(receipt.facility.clone())
            .or_default()
            .insert(receipt.servicer.clone());
    }
    let mut guns = BTreeMap::<String, (FacilityId, BTreeMap<SgsuId, Vec<PlaneId>>)>::new();
    let mut selected_ids = BTreeSet::new();
    for cohort in &plan.arming {
        if cohort.planes.is_empty() {
            continue;
        }
        let unit = state
            .air
            .runtime
            .sgsus
            .get(&cohort.servicer)
            .filter(|s| s.side().ok() == Some(owner))
            .ok_or_else(|| illegal("unknown own arming SGSU"))?;
        let SgsuPosition::Facility(site_id) = &unit.position else {
            return Err(unsupported("airlog:36.17", "arming needs exact facility SGSU").into());
        };
        let site = state
            .air
            .runtime
            .facilities
            .get(site_id)
            .ok_or_else(|| invalid("arming site missing"))?;
        let props = site.properties(content, site_id)?;
        if !site.operational() {
            return Err(illegal("own arming facility is unavailable"));
        }
        if props.theatre != FacilityTheatre::Africa
            || !matches!(props.location, Location::Hex { .. })
        {
            return Err(unsupported(
                "airlog:36.5",
                "arming exceptions need separate source-backed consumers",
            )
            .into());
        }
        let mut receipt = ArmingReceipt {
            period: current,
            servicer: cohort.servicer.clone(),
            facility: site_id.clone(),
            planes: Vec::new(),
            gun_loads: Vec::new(),
        };
        for selected in &cohort.planes {
            if !selected_ids.insert(selected.plane.clone()) {
                return Err(illegal("duplicate arming PlaneId"));
            }
            let plane = state
                .air
                .runtime
                .aircraft
                .get(&selected.plane)
                .filter(|p| owned(&p.force, owner))
                .ok_or_else(|| illegal("unknown own arming plane"))?;
            if crate::air::operations::ground_facility(content, state, &selected.plane)?.as_ref() != Some(site_id) {
                return Err(illegal("plane actual arming site differs"));
            }
            let squadron = plane
                .squadron
                .as_ref()
                .ok_or_else(|| illegal("arming plane has no squadron"))?;
            let aircraft = content
                .units
                .aircraft
                .get(&plane.aircraft)
                .ok_or_else(|| invalid("arming aircraft source missing"))?;
            if !props.kind.accepts(aircraft.role == "flying_boat") {
                return Err(illegal("arming aircraft incompatible with facility"));
            }
            if selected.guns == GunSelection::Preserve && selected.bombs.is_none() {
                continue;
            }
            let before = state
                .air
                .runtime
                .preparation
                .current
                .get(&selected.plane)
                .ok_or_else(|| invalid("arming preparation missing"))?
                .clone();
            let base = fuel
                .changes
                .get(&selected.plane)
                .map_or(&before, |c| &c.after);
            let mut next = KnownAmmunition::from_current(base).ok_or_else(|| {
                unsupported(
                    "scen:59.32",
                    "initial aircraft needs its complete joint load choice, not partial arming",
                )
            })?;
            let new_guns = selected.guns == GunSelection::Load && next.gun_ammunition != Some(true);
            let mut new_bombs = false;
            if let Some(BombLoad { mode, bomb_points }) = &selected.bombs {
                if next.mode.is_some_and(|known| known != *mode) {
                    return Err(illegal(
                        "arming cannot change known or simultaneous fuel mode",
                    ));
                }
                let source = aircraft
                    .modes
                    .get(*mode)
                    .ok_or_else(|| illegal("bomb mode outside aircraft source"))?;
                let limit = source.bomb_capacity.ok_or_else(|| {
                    unsupported("airlog:34.14", "selected mode bomb rating missing")
                })?;
                if limit < 0 {
                    return Err(invalid("negative source bomb rating").into());
                }
                if *bomb_points <= 0 || *bomb_points > limit {
                    return Err(illegal("bomb payload outside positive source rating"));
                }
                let payload = OrdnanceKnowledge::Known {
                    carried: Some(CarriedOrdnance::Bombs {
                        bomb_points: *bomb_points,
                    }),
                };
                new_bombs = next.ordnance != payload;
                if new_bombs {
                    next.ordnance = payload;
                    next.mode = Some(*mode);
                }
            }
            if !new_guns && !new_bombs {
                continue;
            }
            // A proposed fuel update must not hide the original known mode
            // from this slice's unchanged-mode restriction on actual arming.
            if KnownAmmunition::from_current(&before)
                .and_then(|known| known.mode)
                .is_some_and(|mode| next.mode != Some(mode))
            {
                return Err(illegal("arming cannot change original known mode"));
            }
            if first_bar(content, current) {
                return Err(illegal(
                    "scen:59.36: aircraft maintenance barred in first scenario OpStage",
                ));
            }
            if !unit.operation_paid(dues_stage(current)?) && !plan.dues.contains(&cohort.servicer) {
                return Err(illegal("arming SGSU dues not paid for actual period"));
            }
            let family = designation::squadron_family(state, squadron).ok_or_else(|| {
                unsupported(
                    "airlog:38.42",
                    "arming maintenance class has no committed designation",
                )
            })?;
            if matches!(
                (current, family),
                (
                    MaintenancePeriod::Tactical { .. },
                    Family::Convoy | Family::MaltaRaid
                ) | (MaintenancePeriod::Strategic { .. }, Family::LandSupport)
            ) {
                return Err(illegal(
                    "arming maintenance class differs from fixed period",
                ));
            }
            if !seen.insert(selected.plane.clone()) {
                return Err(illegal("PlaneId already armed in this maintenance period"));
            }
            if new_guns {
                // Binding uses the recorded charge site, never the current site
                // of a historical servicing SGSU that may have moved or vanished.
                if charged.get(squadron).is_some_and(|old| old != site_id) {
                    return Err(unsupported(
                        "airlog:38.43",
                        "historical cross-site squadron gun payer unresolved",
                    )
                    .into());
                }
                let group = guns
                    .entry(squadron.clone())
                    .or_insert_with(|| (site_id.clone(), BTreeMap::new()));
                if group.0 != *site_id {
                    return Err(unsupported(
                        "airlog:38.43",
                        "cross-site squadron gun payer unresolved",
                    )
                    .into());
                }
                group
                    .1
                    .entry(cohort.servicer.clone())
                    .or_default()
                    .push(selected.plane.clone());
                receipt
                    .gun_loads
                    .push((selected.plane.clone(), squadron.clone()));
                next.gun_ammunition = Some(true);
            }
            let demand = result.demands.entry(cohort.servicer.clone()).or_default();
            if new_bombs {
                demand.ammo = AmmoPoints::new(
                    demand
                        .ammo
                        .get()
                        .checked_add(1)
                        .ok_or_else(|| invalid("bomb ammunition overflow"))?,
                );
            }
            let after = next.into_current();
            result.changes.insert(
                selected.plane.clone(),
                ServicingChange {
                    plane: plane.clone(),
                    before,
                    after,
                },
            );
            receipt.planes.push(selected.plane.clone());
            *counts.entry(cohort.servicer.clone()).or_default() += 1;
            result
                .active
                .entry(site_id.clone())
                .or_default()
                .insert(cohort.servicer.clone());
        }
        if !receipt.planes.is_empty() {
            result.receipts.push(receipt);
        }
    }
    for (squadron, (facility, loaders)) in guns {
        if charged.contains_key(&squadron) {
            continue;
        }
        let payer = loaders
            .keys()
            .next()
            .ok_or_else(|| invalid("empty gun charge loaders"))?
            .clone();
        let d = result.demands.entry(payer.clone()).or_default();
        d.ammo = AmmoPoints::new(
            d.ammo
                .get()
                .checked_add(1)
                .ok_or_else(|| invalid("gun ammunition overflow"))?,
        );
        result.charges.push(GunCharge {
            period: current,
            assigned_squadron: squadron,
            facility,
            payer,
            planes: loaders.values().flatten().cloned().collect(),
        });
    }
    for (id, count) in counts {
        let unit = state
            .air
            .runtime
            .sgsus
            .get(&id)
            .filter(|s| s.side().ok() == Some(owner))
            .ok_or_else(|| invalid("arming history SGSU missing"))?;
        if count
            > usize::try_from(capacity(content, state, &unit.nationality)?)
                .map_err(|_| invalid("arming capacity overflow"))?
        {
            return Err(illegal("arming service exceeds SGSU ready+reserve"));
        }
    }
    Ok(result)
}
