//! Finite actual-SGSU fuel service. No free load, ammunition or flight rights.
use super::*;
use crate::air::{
    preparation::{CurrentPreparation, KnownLoad},
    state::{AirRuntime, AircraftState},
};
use refit::FuelReceipt;

#[derive(Clone)]
pub(super) struct ServicingChange {
    pub(super) plane: AircraftState,
    pub(super) before: CurrentPreparation,
    pub(super) after: CurrentPreparation,
}

#[derive(Clone)]
pub(super) struct ValidatedFueling {
    pub(super) changes: BTreeMap<PlaneId, ServicingChange>,
    pub(super) demands: BTreeMap<SgsuId, SupplyDemand>,
    pub(super) active: BTreeMap<FacilityId, BTreeSet<SgsuId>>,
    pub(super) receipts: Vec<FuelReceipt>,
}

impl ValidatedFueling {
    pub(super) fn add_demands(
        &self,
        demands: &mut BTreeMap<SgsuId, SupplyDemand>,
    ) -> Result<(), EngineError> {
        for (id, fuel) in &self.demands {
            let demand = demands.entry(id.clone()).or_default();
            demand.fuel = FuelTenths::new(
                demand
                    .fuel
                    .get()
                    .checked_add(fuel.fuel.get())
                    .ok_or_else(|| invalid("fuel service demand overflow"))?,
            );
        }
        Ok(())
    }
}

/// Only the maintenance parent can construct this, after its combined draft
/// debit. Validation alone returns no permission. No serialized token exists.
pub(in crate::air) struct PaidServicingAdmission {
    owner: Side,
    period: MaintenancePeriod,
    changes: BTreeMap<PlaneId, ServicingChange>,
    receipts_before: Vec<super::arming::ArmingReceipt>,
    charges_before: Vec<super::arming::GunCharge>,
    receipts: Vec<super::arming::ArmingReceipt>,
    charges: Vec<super::arming::GunCharge>,
}

fn after_debit(
    state: &State,
    owner: Side,
    period: MaintenancePeriod,
    changes: BTreeMap<PlaneId, ServicingChange>,
    arming: &super::arming::ValidatedArming,
) -> Result<PaidServicingAdmission, EngineError> {
    if state.air.runtime.maintenance.period != Some(period)
        || !state.air.runtime.maintenance.closed.contains(&owner)
        || state.air.runtime.maintenance.finished
    {
        return Err(invalid(
            "paid servicing admission has no matching closed period",
        ));
    }
    let admission = PaidServicingAdmission {
        owner,
        period,
        changes,
        receipts_before: state
            .air
            .runtime
            .maintenance
            .arming_receipts
            .get(&owner)
            .cloned()
            .unwrap_or_default(),
        charges_before: state
            .air
            .runtime
            .maintenance
            .gun_charges
            .get(&owner)
            .cloned()
            .unwrap_or_default(),
        receipts: arming.receipts.clone(),
        charges: arming.charges.clone(),
    };
    admission.verify(&state.air.runtime)?;
    Ok(admission)
}

/// Shared exact maintenance debit; only a successful spend can return a fuel
/// admission. All plans were already validated before this finish-only call.
/// Cases: airlog:35.14, airlog:38.21, airlog:38.24, airlog:38.36
pub(super) fn spend_and_admit(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    current: MaintenancePeriod,
    validated: &mut Validated,
) -> Result<Option<PaidServicingAdmission>, EngineError> {
    for receipt in state
        .air
        .runtime
        .maintenance
        .debits
        .get(&side)
        .into_iter()
        .flatten()
    {
        for (source, amount) in &receipt.fuel_rounding {
            let total = validated.prior.entry(source.clone()).or_default();
            *total = FuelTenths::new(
                total
                    .get()
                    .checked_add(amount.get())
                    .ok_or_else(|| invalid("finite refit rounding overflow"))?,
            );
        }
    }
    for (id, demand) in &validated.demands {
        let draws = validated
            .plans
            .draws
            .iter()
            .find(|d| &d.servicer == id)
            .map(|d| d.draws.as_slice())
            .unwrap_or(&[]);
        let use_ = if validated.plans.dues.contains(id) {
            AirSupplyUse::SgsuOperation
        } else {
            AirSupplyUse::AircraftServicing
        };
        air_supply::spend(
            content,
            state,
            side,
            id,
            use_,
            AirSupplyDebit {
                demand: *demand,
                draws,
                prior: &validated.prior,
            },
        )
        .map_err(supply_error)
        .map_err(adjudication_error)?;
        for draw in draws {
            let total = validated.prior.entry(draw.source.clone()).or_default();
            *total = FuelTenths::new(
                total
                    .get()
                    .checked_add(draw.amount.fuel.get())
                    .ok_or_else(|| invalid("finite refit rounding overflow"))?,
            );
        }
    }
    let mut changes = validated.fueling.changes.clone();
    for (id, change) in &validated.arming.changes {
        if let Some(fuel) = changes.get(id)
            && (fuel.before != change.before || fuel.plane != change.plane)
        {
            return Err(invalid("simultaneous service before-record differs"));
        }
        changes.insert(id.clone(), change.clone());
    }
    if changes.is_empty() {
        return Ok(None);
    }
    after_debit(state, side, current, changes, &validated.arming).map(Some)
}

impl PaidServicingAdmission {
    fn verify(&self, runtime: &AirRuntime) -> Result<(), EngineError> {
        if runtime.maintenance.period != Some(self.period)
            || !runtime.maintenance.closed.contains(&self.owner)
            || runtime.maintenance.finished
        {
            return Err(invalid("paid servicing admission period changed"));
        }
        if runtime
            .maintenance
            .arming_receipts
            .get(&self.owner)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            != self.receipts_before.as_slice()
            || runtime
                .maintenance
                .gun_charges
                .get(&self.owner)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                != self.charges_before.as_slice()
        {
            return Err(invalid("paid servicing history changed"));
        }
        for (id, change) in &self.changes {
            if runtime.aircraft.get(id) != Some(&change.plane)
                || runtime.preparation.current.get(id) != Some(&change.before)
                || !owned(&change.plane.force, self.owner)
            {
                return Err(invalid("paid servicing admission identities changed"));
            }
        }
        Ok(())
    }

    /// No callback: only the exact paid final servicing facts may change.
    pub(in crate::air) fn apply(&self, runtime: &mut AirRuntime) -> Result<(), EngineError> {
        self.verify(runtime)?;
        if !self.receipts.is_empty() {
            runtime
                .maintenance
                .arming_receipts
                .entry(self.owner)
                .or_default()
                .extend(self.receipts.clone());
        }
        if !self.charges.is_empty() {
            runtime
                .maintenance
                .gun_charges
                .entry(self.owner)
                .or_default()
                .extend(self.charges.clone());
        }
        for (id, change) in &self.changes {
            runtime
                .preparation
                .current
                .insert(id.clone(), change.after.clone());
            let plane = runtime
                .aircraft
                .get_mut(id)
                .ok_or_else(|| invalid("paid plane disappeared"))?;
            plane.fuelled = crate::air::preparation::fuel_availability(&change.after)
                .unwrap_or(change.plane.fuelled);
            plane.armed =
                crate::air::preparation::armed_projection(&change.after, change.plane.armed);
        }
        Ok(())
    }

    pub(in crate::air) fn permits_history(&self, old: &AirRuntime, new: &AirRuntime) -> bool {
        let mut receipts = old.maintenance.arming_receipts.clone();
        let mut charges = old.maintenance.gun_charges.clone();
        if !self.receipts.is_empty() {
            receipts
                .entry(self.owner)
                .or_default()
                .extend(self.receipts.clone());
        }
        if !self.charges.is_empty() {
            charges
                .entry(self.owner)
                .or_default()
                .extend(self.charges.clone());
        }
        old.maintenance
            .arming_receipts
            .get(&self.owner)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            == self.receipts_before.as_slice()
            && old
                .maintenance
                .gun_charges
                .get(&self.owner)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                == self.charges_before.as_slice()
            && new.maintenance.arming_receipts == receipts
            && new.maintenance.gun_charges == charges
    }

    pub(in crate::air) fn permits(&self, id: &PlaneId, old: &AirRuntime, new: &AirRuntime) -> bool {
        let Some(change) = self.changes.get(id) else {
            return false;
        };
        let mut expected = change.plane.clone();
        expected.fuelled = crate::air::preparation::fuel_availability(&change.after)
            .unwrap_or(change.plane.fuelled);
        expected.armed =
            crate::air::preparation::armed_projection(&change.after, change.plane.armed);
        old.maintenance.period == Some(self.period)
            && old.maintenance.closed.contains(&self.owner)
            && old.aircraft.get(id) == Some(&change.plane)
            && new.aircraft.get(id) == Some(&expected)
            && old.preparation.current.get(id) == Some(&change.before)
            && new.preparation.current.get(id) == Some(&change.after)
    }
}

/// Owner-only legality/cost checks; no payment, migration, RNG or admission.
/// Cases: airlog:38.21, airlog:38.22, airlog:38.23, airlog:38.24, airlog:38.25
pub(super) fn validate(
    content: &CnaContent,
    state: &State,
    owner: Side,
    current: MaintenancePeriod,
    plan: &RefitPlan,
) -> Result<ValidatedFueling, Rejection> {
    let mut result = ValidatedFueling {
        changes: BTreeMap::new(),
        demands: BTreeMap::new(),
        active: BTreeMap::new(),
        receipts: Vec::new(),
    };
    let mut seen = BTreeSet::new();
    let mut counts = BTreeMap::<SgsuId, usize>::new();
    for receipt in state
        .air
        .runtime
        .maintenance
        .fuel_receipts
        .get(&owner)
        .into_iter()
        .flatten()
        .filter(|r| r.period == current)
    {
        let unit = state
            .air
            .runtime
            .sgsus
            .get(&receipt.cohort.servicer)
            .filter(|s| s.side().ok() == Some(owner))
            .ok_or_else(|| invalid("fuel history SGSU missing"))?;
        let SgsuPosition::Facility(site_id) = &unit.position else {
            return Err(invalid("fuel history SGSU has no facility").into());
        };
        result
            .active
            .entry(site_id.clone())
            .or_default()
            .insert(receipt.cohort.servicer.clone());
        for selection in &receipt.cohort.planes {
            if !seen.insert(selection.plane.clone()) {
                return Err(invalid("duplicate fuel history").into());
            }
        }
        *counts.entry(receipt.cohort.servicer.clone()).or_default() += receipt.cohort.planes.len();
    }
    for cohort in &plan.fueling {
        if cohort.planes.is_empty() {
            continue;
        }
        if first_bar(content, current) {
            return Err(illegal(
                "scen:59.36: aircraft maintenance barred in first scenario OpStage",
            ));
        }
        let unit = state
            .air
            .runtime
            .sgsus
            .get(&cohort.servicer)
            .filter(|s| s.side().ok() == Some(owner))
            .ok_or_else(|| illegal("unknown own fueling SGSU"))?;
        let SgsuPosition::Facility(site_id) = &unit.position else {
            return Err(
                unsupported("airlog:36.17", "finite fuel needs exact facility SGSU").into(),
            );
        };
        let site = state
            .air
            .runtime
            .facilities
            .get(site_id)
            .ok_or_else(|| invalid("fuel SGSU site missing"))?;
        let props = site.properties(content, site_id)?;
        if !site.operational() {
            return Err(illegal("own fueling facility is unavailable"));
        }
        if props.theatre != FacilityTheatre::Africa
            || !matches!(props.location, Location::Hex { .. })
        {
            return Err(unsupported(
                "airlog:36.5",
                "fuel exceptions require a separate source-backed producer",
            )
            .into());
        }
        if !unit.operation_paid(dues_stage(current)?) && !plan.dues.contains(&cohort.servicer) {
            return Err(illegal("fueling SGSU dues not paid for actual period"));
        }
        result
            .active
            .entry(site_id.clone())
            .or_default()
            .insert(cohort.servicer.clone());
        for selected in &cohort.planes {
            let plane = state
                .air
                .runtime
                .aircraft
                .get(&selected.plane)
                .filter(|p| owned(&p.force, owner))
                .ok_or_else(|| illegal("unknown own fuel plane"))?;
            if crate::air::operations::ground_facility(content, state, &selected.plane)?.as_ref()
                != Some(site_id)
            {
                return Err(illegal("plane actual fuel site differs"));
            }
            let squadron = plane
                .squadron
                .as_ref()
                .ok_or_else(|| illegal("fuel plane has no assigned squadron"))?;
            let family = designation::squadron_family(state, squadron).ok_or_else(|| {
                unsupported(
                    "airlog:38.24",
                    "fuel maintenance class has no committed designation",
                )
            })?;
            if matches!(
                (current, family),
                (
                    MaintenancePeriod::Tactical { .. },
                    Family::Convoy | Family::MaltaRaid
                ) | (MaintenancePeriod::Strategic { .. }, Family::LandSupport)
            ) {
                return Err(illegal("fuel maintenance class differs from fixed period"));
            }
            let aircraft = content
                .units
                .aircraft
                .get(&plane.aircraft)
                .ok_or_else(|| invalid("fuel aircraft source missing"))?;
            if !props.kind.accepts(aircraft.role == "flying_boat") {
                return Err(illegal("fuel aircraft incompatible with facility"));
            }
            if selected.mode >= aircraft.modes.len() {
                return Err(illegal("fuel mode outside own aircraft source"));
            }
            let points = super::super::fuel_points(aircraft, selected.mode)?;
            let amount = points
                .checked_mul(10)
                .ok_or_else(|| invalid("fuel rating overflow"))?;
            let before = state
                .air
                .runtime
                .preparation
                .current
                .get(&selected.plane)
                .ok_or_else(|| invalid("fuel plane preparation missing"))?
                .clone();
            let after = match &before {
                CurrentPreparation::UnknownLegacy | CurrentPreparation::KnownFuel { .. } => {
                    CurrentPreparation::KnownFuel {
                        mode: selected.mode,
                        fuel_available: true,
                    }
                }
                CurrentPreparation::Unprepared => CurrentPreparation::KnownLoad {
                    load: KnownLoad {
                        mode: selected.mode,
                        fuel_available: true,
                        gun_ammunition: false,
                        ordnance: None,
                    },
                },
                CurrentPreparation::KnownLoad { load } => {
                    if selected.mode != load.mode {
                        return Err(illegal("paid known-load fueling cannot change mode"));
                    }
                    let mut next = load.clone();
                    next.fuel_available = true;
                    CurrentPreparation::KnownLoad { load: next }
                }
                CurrentPreparation::KnownAmmunition { knowledge } => {
                    if knowledge.mode.is_some_and(|mode| mode != selected.mode) {
                        return Err(illegal("fueling cannot change known ammunition mode"));
                    }
                    let mut next = knowledge.clone();
                    next.mode = Some(selected.mode);
                    next.fuel_available = Some(true);
                    next.into_current()
                }
                CurrentPreparation::InitialUnspecified => return Err(unsupported(
                    "scen:59.32",
                    "initial aircraft requires its complete joint load choice, not partial fueling",
                )
                .into()),
            };
            if !seen.insert(selected.plane.clone()) {
                return Err(illegal("PlaneId already fueled in this maintenance period"));
            }
            *counts.entry(cohort.servicer.clone()).or_default() += 1;
            let demand = result.demands.entry(cohort.servicer.clone()).or_default();
            demand.fuel = FuelTenths::new(
                demand
                    .fuel
                    .get()
                    .checked_add(amount)
                    .ok_or_else(|| invalid("fuel demand overflow"))?,
            );
            result.changes.insert(
                selected.plane.clone(),
                ServicingChange {
                    plane: plane.clone(),
                    before,
                    after,
                },
            );
        }
        result.receipts.push(FuelReceipt {
            period: current,
            cohort: cohort.clone(),
        });
    }
    for (id, count) in counts {
        let unit = state
            .air
            .runtime
            .sgsus
            .get(&id)
            .ok_or_else(|| invalid("fuel SGSU missing"))?;
        if count
            > usize::try_from(capacity(content, state, &unit.nationality)?)
                .map_err(|_| invalid("fuel capacity overflow"))?
        {
            return Err(illegal("fuel service exceeds SGSU ready+reserve"));
        }
    }
    Ok(result)
}
