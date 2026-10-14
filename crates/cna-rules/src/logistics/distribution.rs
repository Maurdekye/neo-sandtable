//! Same-location supply transfers. Truck cargo is unloaded before another truck loads it.
//! This procedure handles the free Supply Distribution window only. Other windows must
//! charge loading CP and preserve the no-leapfrogging cargo ledger before using this API.
//! Cargo handling at the four off-map supply boxes stamps an OpStage movement ban.
use super::stores::field;
use super::{CargoPacking, SupplyError, capacity, fuel_capacity, validate_packing};
use crate::{
    CnaContent,
    state::{DumpLocation, Location, Pending, State},
    steps::illegal,
};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_core::{
    decision::ActionSchema,
    engine::{Cx, EngineError, Rejection},
    ids::UnitId,
    quantity::FuelTenths,
};
use cna_protocol::Side;
use cna_tables::airlog::supply::{DumpCapacity, DumpLocation as ChartLocation, SupplyType};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const KIND: &str = "cna.logistics.distribution";
pub const PREFIX: &str = "cna.logistics.distribution.to:";
const TYPES: [SupplyType; 4] = [
    SupplyType::Ammo,
    SupplyType::Fuel,
    SupplyType::Stores,
    SupplyType::Water,
];

/// Stable owner-only transfer identities; tanks and ready ammo are receivers, not sources.
/// Cases: airlog:49.16, airlog:49.17, airlog:50.15, airlog:50.17, airlog:53.24
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Endpoint {
    Dump(String),
    Cargo(UnitId),
    Pool(String),
    Tank(UnitId),
    Ready(UnitId),
    Ground(Location),
}

/// Cases: airlog:53.24, airlog:54.11
pub fn location(state: &State, side: Side, end: &Endpoint) -> Result<Location, SupplyError> {
    match end {
        Endpoint::Ground(at) => {
            if matches!(at, Location::Hex { .. })
                && (state
                    .land
                    .units
                    .values()
                    .any(|u| u.side == side && &u.location == at)
                    || state
                        .logistics
                        .truck_pools
                        .iter()
                        .any(|p| p.side == side && p.location.as_ref() == Some(at)))
            {
                Ok(at.clone())
            } else {
                Err(SupplyError::Invalid)
            }
        }
        Endpoint::Dump(id) => {
            let d = state
                .logistics
                .dumps
                .get(id)
                .filter(|d| d.side == side && d.active && !d.dummy)
                .ok_or(SupplyError::Invalid)?;
            match &d.location {
                DumpLocation::Hex { hex } => Ok(Location::Hex { hex: hex.clone() }),
                DumpLocation::OffMap { id } => Ok(Location::OffMap { id: id.clone() }),
                _ => Err(SupplyError::Invalid),
            }
        }
        Endpoint::Cargo(id) | Endpoint::Tank(id) | Endpoint::Ready(id) => state
            .land
            .units
            .get(id)
            .filter(|u| {
                u.side == side
                    && matches!(u.location, Location::Hex { .. } | Location::OffMap { .. })
            })
            .map(|u| u.location.clone())
            .ok_or(SupplyError::Invalid),
        Endpoint::Pool(id) => state
            .logistics
            .truck_pools
            .iter()
            .find(|p| &p.id == id && p.side == side)
            .and_then(|p| p.location.clone())
            .filter(|l| matches!(l, Location::Hex { .. } | Location::OffMap { .. }))
            .ok_or(SupplyError::Invalid),
    }
}
pub(super) fn stock(state: &State, end: &Endpoint) -> Result<Supplies, SupplyError> {
    Ok(match end {
        Endpoint::Dump(id) => {
            state
                .logistics
                .dumps
                .get(id)
                .ok_or(SupplyError::Invalid)?
                .supplies
        }
        Endpoint::Cargo(id) => state
            .logistics
            .unit_supply
            .get(id)
            .map_or(Supplies::default(), |h| h.carried),
        Endpoint::Pool(id) => {
            state
                .logistics
                .truck_pools
                .iter()
                .find(|p| &p.id == id)
                .ok_or(SupplyError::Invalid)?
                .cargo
        }
        Endpoint::Tank(_) | Endpoint::Ready(_) | Endpoint::Ground(_) => {
            return Err(SupplyError::Invalid);
        }
    })
}
fn set_stock(state: &mut State, end: &Endpoint, s: Supplies) {
    match end {
        Endpoint::Dump(id) => state.logistics.dumps.get_mut(id).unwrap().supplies = s,
        Endpoint::Cargo(id) => {
            state
                .logistics
                .unit_supply
                .entry(id.clone())
                .or_default()
                .carried = s
        }
        Endpoint::Pool(id) => {
            state
                .logistics
                .truck_pools
                .iter_mut()
                .find(|p| &p.id == id)
                .unwrap()
                .cargo = s
        }
        Endpoint::Tank(_) | Endpoint::Ready(_) | Endpoint::Ground(_) => unreachable!(),
    }
}
fn truck(end: &Endpoint) -> bool {
    matches!(end, Endpoint::Cargo(_) | Endpoint::Pool(_))
}
pub(super) fn endpoints(state: &State, side: Side) -> Vec<Endpoint> {
    let mut out = Vec::new();
    out.extend(
        state
            .logistics
            .dumps
            .values()
            .filter(|d| d.side == side && d.active && !d.dummy)
            .map(|d| Endpoint::Dump(d.id.clone())),
    );
    for u in state.land.units.values().filter(|u| u.side == side) {
        if u.trucks.light + u.trucks.medium + u.trucks.heavy > 0 {
            out.push(Endpoint::Cargo(u.id.clone()));
        }
        out.push(Endpoint::Tank(u.id.clone()));
        out.push(Endpoint::Ready(u.id.clone()));
    }
    out.extend(
        state
            .logistics
            .truck_pools
            .iter()
            .filter(|p| p.side == side)
            .map(|p| Endpoint::Pool(p.id.clone())),
    );
    let grounds: Vec<_> = out
        .iter()
        .filter_map(|e| location(state, side, e).ok())
        .filter(|at| matches!(at, Location::Hex { .. }))
        .collect();
    for at in grounds {
        let end = Endpoint::Ground(at);
        if !out.contains(&end) {
            out.push(end)
        }
    }
    out.into_iter()
        .filter(|e| location(state, side, e).is_ok())
        .collect()
}
pub(super) fn sources(state: &State, side: Side, to: &Endpoint) -> Vec<Endpoint> {
    let Ok(at) = location(state, side, to) else {
        return vec![];
    };
    endpoints(state, side)
        .into_iter()
        .filter(|e| {
            e != to
                && !matches!(
                    e,
                    Endpoint::Tank(_) | Endpoint::Ready(_) | Endpoint::Ground(_)
                )
                && !(truck(e) && truck(to))
                && !(matches!(e, Endpoint::Pool(_))
                    && matches!(to, Endpoint::Tank(_) | Endpoint::Ready(_)))
                && location(state, side, e).is_ok_and(|l| l == at)
                && stock(state, e).is_ok_and(|s| {
                    if matches!(to, Endpoint::Tank(_)) {
                        s.fuel > 0
                    } else if matches!(to, Endpoint::Ready(_)) {
                        s.ammo > 0
                    } else {
                        TYPES.into_iter().any(|t| capacity::points(&s, t) > 0)
                    }
                })
        })
        .collect()
}

/// Aggregate all real friendly dumps at this location, so extra counters cannot expand
/// hex capacity. With incomplete settlement data only a bound valid for every possible
/// dump classification is accepted; increases above it require verified membership.
/// Cases: airlog:54.11, airlog:54.13
pub fn validate_dump_capacity(
    content: &CnaContent,
    state: &State,
    side: Side,
    at: &Location,
    addition: &Supplies,
) -> Result<(), SupplyError> {
    let mut total = Supplies::default();
    for d in state
        .logistics
        .dumps
        .values()
        .filter(|d| d.side == side && d.active && !d.dummy)
    {
        let loc = match &d.location {
            DumpLocation::Hex { hex } => Location::Hex { hex: hex.clone() },
            DumpLocation::OffMap { id } => Location::OffMap { id: id.clone() },
            _ => continue,
        };
        if &loc == at {
            for t in TYPES {
                let n = capacity::points(&total, t)
                    .checked_add(capacity::points(&d.supplies, t))
                    .ok_or(SupplyError::Invalid)?;
                capacity::set_points(&mut total, t, n);
            }
        }
    }
    let class = match at {
        Location::OffMap { id }
            if ["box_tripoli", "box_tripolitania", "box_tunis", "box_gabes"]
                .contains(&id.as_str()) =>
        {
            Some(ChartLocation::TunisTripoli)
        }
        Location::Hex { hex } if content.places.at(hex).any(|p| p.kind == "major_city") => {
            Some(ChartLocation::MajorCity)
        }
        Location::Hex { hex }
            if content
                .places
                .at(hex)
                .any(|p| p.kind == "village" || p.kind == "town") =>
        {
            Some(ChartLocation::Village)
        }
        Location::Hex { .. } => None,
        _ => {
            return Err(SupplyError::Unsupported {
                case: "airlog:54.13",
            });
        }
    };
    for t in TYPES {
        let n = capacity::points(addition, t);
        if n < 0 {
            return Err(SupplyError::Invalid);
        }
        if n == 0 {
            continue;
        }
        let sum = capacity::points(&total, t)
            .checked_add(n)
            .ok_or(SupplyError::Invalid)?;
        let cap = content
            .tables
            .airlog
            .supply_dump_capacity
            .capacity(class.unwrap_or(ChartLocation::OtherTerrain), t);
        if matches!(cap,DumpCapacity::Max(max) if sum>max) {
            return Err(if class.is_none() {
                SupplyError::Unsupported {
                    case: "airlog:54.13",
                }
            } else {
                SupplyError::Insufficient
            });
        }
    }
    Ok(())
}

/// Atomically load/unload one friendly same-location stock. For a tank receiver amount.fuel
/// is in tenths, and the whole-point cargo source pays the ceiling exactly once. For all
/// other receivers every amount is in whole points. Pools must unload to a dump first.
/// Cases: airlog:49.14, airlog:49.16, airlog:50.15, airlog:53.24, airlog:54.13, airlog:54.2, land:8.88
/// Interpretations: interp:airlog-0001, interp:airlog-0008, interp:airlog-0019
#[allow(clippy::too_many_arguments)]
fn transfer_stock_only(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    from: &Endpoint,
    to: &Endpoint,
    amount: Supplies,
    packing: &CargoPacking,
) -> Result<(), SupplyError> {
    if state.cursor.anchor() != "opstage.organization.supply_distribution" || from == to {
        return Err(SupplyError::Invalid);
    }
    let at = location(state, side, from)?;
    if at != location(state, side, to)?
        || (truck(from) && truck(to))
        || (matches!(from, Endpoint::Pool(_))
            && matches!(to, Endpoint::Tank(_) | Endpoint::Ready(_)))
    {
        return Err(SupplyError::Invalid);
    }
    if TYPES.into_iter().any(|t| capacity::points(&amount, t) < 0)
        || TYPES.into_iter().all(|t| capacity::points(&amount, t) == 0)
    {
        return Err(SupplyError::Invalid);
    }
    let mut debit = amount;
    let mut draft = state.clone();
    let target = if let Endpoint::Ground(at) = to {
        let mut n = 1u64;
        let id = loop {
            let id = format!("{}.dump-{n}", crate::state::side_key(side));
            if !draft.logistics.dumps.contains_key(&id) {
                break id;
            }
            n = n.checked_add(1).ok_or(SupplyError::Invalid)?;
        };
        let Location::Hex { hex } = at else {
            return Err(SupplyError::Invalid);
        };
        let marker = super::dump_markers::next_marker(&mut draft.logistics)?;
        draft.logistics.dumps.insert(
            id.clone(),
            crate::state::Dump {
                marker,
                id: id.clone(),
                side,
                location: DumpLocation::Hex { hex: hex.clone() },
                supplies: Supplies::default(),
                active: true,
                dummy: false,
            },
        );
        Endpoint::Dump(id)
    } else {
        to.clone()
    };
    let to = &target;
    if let Endpoint::Ready(id) = to {
        if amount.fuel != 0 || amount.stores != 0 || amount.water != 0 {
            return Err(SupplyError::Invalid);
        }
        let cap = super::ready_ammo_capacity(content, &draft, id)?;
        let holding = draft.logistics.unit_supply.entry(id.clone()).or_default();
        let new = holding
            .ready_ammo
            .get()
            .checked_add(amount.ammo)
            .ok_or(SupplyError::Invalid)?;
        if new > cap.get() {
            return Err(SupplyError::Insufficient);
        }
        holding.ready_ammo = cna_core::quantity::AmmoPoints::new(new);
    } else if let Endpoint::Tank(id) = to {
        if amount.ammo != 0 || amount.stores != 0 || amount.water != 0 {
            return Err(SupplyError::Invalid);
        }
        let cap = fuel_capacity(content, &draft, id)?;
        let holding = draft.logistics.unit_supply.entry(id.clone()).or_default();
        let new = holding
            .tank_fuel
            .get()
            .checked_add(amount.fuel)
            .ok_or(SupplyError::Invalid)?;
        if new > cap.get() {
            return Err(SupplyError::Insufficient);
        }
        holding.tank_fuel = FuelTenths::new(new);
        debit.fuel =
            i32::try_from((i64::from(amount.fuel) + 9) / 10).map_err(|_| SupplyError::Invalid)?;
    } else {
        let mut target = stock(&draft, to)?;
        for t in TYPES {
            let n = capacity::points(&target, t)
                .checked_add(capacity::points(&amount, t))
                .ok_or(SupplyError::Invalid)?;
            capacity::set_points(&mut target, t, n);
        }
        match to {
            Endpoint::Dump(_) => validate_dump_capacity(
                content,
                state,
                side,
                &at,
                &if matches!(from, Endpoint::Dump(_)) {
                    Supplies::default()
                } else {
                    amount
                },
            )?,
            Endpoint::Cargo(id) => {
                let u = &state.land.units[id];
                validate_packing(content, &u.trucks, &u.transport_trucks, &target, packing)?;
            }
            Endpoint::Pool(id) => {
                let p = state
                    .logistics
                    .truck_pools
                    .iter()
                    .find(|p| &p.id == id)
                    .unwrap();
                validate_packing(content, &p.trucks, &Trucks::default(), &target, packing)?;
            }
            Endpoint::Tank(_) | Endpoint::Ready(_) | Endpoint::Ground(_) => unreachable!(),
        }
        set_stock(&mut draft, to, target);
    }
    let mut source = stock(&draft, from)?;
    for t in TYPES {
        let n = capacity::points(&source, t)
            .checked_sub(capacity::points(&debit, t))
            .ok_or(SupplyError::Invalid)?;
        if n < 0 {
            return Err(SupplyError::Insufficient);
        }
        capacity::set_points(&mut source, t, n);
    }
    set_stock(&mut draft, from, source);
    if !matches!(to, Endpoint::Tank(_)) {
        for (endpoint, loading) in [(from, false), (to, true)] {
            let carrier = match endpoint {
                Endpoint::Cargo(id) => Some(super::box_handling::Carrier::Unit(id.clone())),
                Endpoint::Pool(id) => Some(super::box_handling::Carrier::Pool(id.clone())),
                _ => None,
            };
            if let Some(carrier) = carrier {
                super::box_handling::record_goods(&mut draft, &carrier, amount, loading)?;
            }
        }
    }
    *state = draft;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistributionCargoError {
    History(super::cargo_history::CargoError),
    Motion(super::cargo_history::motion::MotionError),
    InvalidCarrier(&'static str),
    UnintegratedRoute(&'static str),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistributionError {
    Supply(SupplyError),
    Cargo(DistributionCargoError),
}
impl From<SupplyError> for DistributionError {
    fn from(error: SupplyError) -> Self {
        Self::Supply(error)
    }
}
impl From<super::cargo_history::CargoError> for DistributionError {
    fn from(error: super::cargo_history::CargoError) -> Self {
        Self::Cargo(DistributionCargoError::History(error))
    }
}
impl From<super::cargo_history::motion::MotionError> for DistributionError {
    fn from(error: super::cargo_history::motion::MotionError) -> Self {
        Self::Cargo(DistributionCargoError::Motion(error))
    }
}
fn invalid_carrier(detail: &'static str) -> DistributionError {
    DistributionError::Cargo(DistributionCargoError::InvalidCarrier(detail))
}
/// Preserve owner action errors and trusted source/history categories independently.
pub(super) fn rejection(error: DistributionError) -> Rejection {
    use super::cargo_history::{CargoError, motion::MotionError};
    let unsupported = |detail: &str| {
        Rejection::Engine(EngineError::Unsupported {
            case: "airlog:53.25".into(),
            detail: detail.into(),
        })
    };
    let invariant = |detail: &str| {
        Rejection::Engine(EngineError::Invariant {
            detail: detail.into(),
        })
    };
    match error {
        DistributionError::Supply(
            error @ (SupplyError::Unsupported { .. } | SupplyError::UnknownFuelRate),
        ) => Rejection::Engine(super::stores::engine(error)),
        DistributionError::Supply(_) => {
            illegal("transfer exceeds stocks, capacity or same-location rules")
        }
        DistributionError::Cargo(DistributionCargoError::Motion(MotionError::Unknown)) => {
            unsupported("organization transfer requires tracked current-stage physical pool timing")
        }
        DistributionError::Cargo(DistributionCargoError::Motion(MotionError::Mixed)) => {
            unsupported("mixed physical pool timing requires a selected packing witness")
        }
        DistributionError::Cargo(DistributionCargoError::History(CargoError::ChoiceRequired)) => {
            unsupported(
                "distinct cargo histories require explicit lot choices in a later handling slice",
            )
        }
        DistributionError::Cargo(DistributionCargoError::History(CargoError::Ceiling)) => {
            illegal("goods exceed their first carrier's CP ceiling (airlog:53.25)")
        }
        DistributionError::Cargo(DistributionCargoError::History(CargoError::Insufficient)) => {
            illegal("cargo parcel quantity is unavailable")
        }
        DistributionError::Cargo(DistributionCargoError::History(CargoError::Invalid)) => {
            invariant("invalid owned distribution cargo history")
        }
        DistributionError::Cargo(DistributionCargoError::Motion(MotionError::Invalid)) => {
            invariant("invalid owned distribution physical timing")
        }
        DistributionError::Cargo(DistributionCargoError::InvalidCarrier(detail)) => {
            invariant(detail)
        }
        DistributionError::Cargo(DistributionCargoError::UnintegratedRoute(detail)) => {
            unsupported(detail)
        }
    }
}
/// No motion/fuel writer: retained physical inventory plus actual stage history only.
fn pool_timing(
    content: &CnaContent,
    state: &State,
    side: Side,
    id: &str,
) -> Result<super::cargo_history::CarrierTiming, DistributionError> {
    use super::{
        cargo_history::{CargoSite, motion},
        segment,
    };
    use cna_tables::airlog::trucks::TruckType;
    // The public wrapper has checked both owner endpoints before trusted records.
    let mut found = state.logistics.truck_pools.iter().filter(|p| p.id == id);
    let p = found.next().ok_or(SupplyError::Invalid)?;
    if p.side != side {
        return Err(SupplyError::Invalid.into());
    }
    if id.is_empty() || found.next().is_some() || !state.logistics.truck_pool_ids.contains(id) {
        return Err(invalid_carrier("invalid real distribution pool identity"));
    }
    let site = CargoSite::Pool(id.into());
    let stage = super::water::WaterStage::current(state);
    let mut current = state
        .logistics
        .cargo_history
        .motion
        .entries
        .iter()
        .filter(|entry| entry.site == site && entry.stage == stage);
    let present = current.next();
    if current.next().is_some() {
        return Err(invalid_carrier(
            "duplicate current distribution pool motion",
        ));
    }
    let physical: Vec<motion::PhysicalTrucks> = if let Some(old) =
        state.logistics.pool_fuel_segments.get(id)
    {
        // Never synthesize initial identities from an uninitialized legacy fuel ledger.
        if !old.cohorts_initialized {
            return Err(if present.is_some() {
                invalid_carrier(
                    "present funded pool motion lacks canonical initialized physical inventory",
                )
            } else {
                motion::MotionError::Unknown.into()
            });
        }
        segment::carrier_ledger(
            Some(old),
            old.segment.clone(),
            &old.origin,
            &p.trucks,
            &id.to_owned(),
            &format!("pool.{id}"),
        )
        .map_err(|_| invalid_carrier("retained distribution pool cohorts do not match inventory"))?
        .cohorts
        .iter()
        .map(motion::PhysicalTrucks::from)
        .collect()
    } else {
        // This requires saved current-stage, exact surviving genesis identities.
        // It creates no groups or zero records and refuses orphan funding accounts.
        super::pool_fuel::unfunded_pool_physical_cohorts(state, id)?
    };
    // Query's Unknown alone is ambiguous. Reconcile every PRESENT current record
    // against the complete canonical physical list before asking for uniform timing.
    let entry = present.ok_or(motion::MotionError::Unknown)?;
    let expected: std::collections::BTreeSet<_> = physical
        .iter()
        .map(|group| (&group.id, group.kind, group.count))
        .collect();
    let actual: std::collections::BTreeSet<_> = entry
        .cohorts
        .iter()
        .map(|group| (&group.id, group.kind, group.count))
        .collect();
    if expected.len() != physical.len()
        || actual.len() != entry.cohorts.len()
        || actual != expected
        || entry
            .cohorts
            .iter()
            .any(|group| group.id.is_empty() || group.count <= 0 || group.spent_cp_quarters < 0)
    {
        return Err(invalid_carrier(
            "present current pool motion does not match full physical inventory",
        ));
    }
    let mut cpa = None;
    for (kind, count) in [
        (TruckType::Light, p.trucks.light),
        (TruckType::Medium, p.trucks.medium),
        (TruckType::Heavy, p.trucks.heavy),
    ] {
        if count < 0 {
            return Err(invalid_carrier("negative distribution pool trucks"));
        }
        if count > 0 {
            let n = content
                .tables
                .airlog
                .truck_characteristics
                .truck(kind)
                .cpa_supplies
                .checked_mul(4)
                .filter(|n| *n > 0)
                .ok_or_else(|| invalid_carrier("invalid bound supply truck CPA"))?;
            cpa = Some(cpa.map_or(n, |old: i32| old.min(n)));
        }
    }
    let cpa = cpa.ok_or(SupplyError::Invalid)?;
    Ok(motion::timing(
        state,
        side,
        &CargoSite::Pool(id.into()),
        &physical,
        cpa,
    )?)
}
/// Current owner parcels only; expiry and malformed-history checks use the
/// existing validated query. Endpoint-wide tagging is intentionally conservative.
fn endpoint_has_tags(
    state: &State,
    side: Side,
    endpoint: &Endpoint,
) -> Result<bool, DistributionError> {
    use super::cargo_history::{self, CargoSite};
    let site = match endpoint {
        Endpoint::Dump(id) => Some(CargoSite::Dump(id.clone())),
        Endpoint::Cargo(id) => Some(CargoSite::Unit(id.clone())),
        Endpoint::Pool(id) => Some(CargoSite::Pool(id.clone())),
        Endpoint::Ground(_) | Endpoint::Tank(_) | Endpoint::Ready(_) => None,
    };
    let Some(site) = site else {
        return Ok(false);
    };
    Ok(cargo_history::parcels(state, side, &site)?
        .iter()
        .any(|parcel| parcel.ceiling_cp_quarters.is_some()))
}
/// Proposed public signature retains arguments and wraps errors locally.
/// ALL original validation precedes tag guards on an unpublished stock draft.
#[allow(clippy::too_many_arguments)]
pub fn transfer(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    from: &Endpoint,
    to: &Endpoint,
    amount: Supplies,
    packing: &CargoPacking,
) -> Result<(), DistributionError> {
    use super::cargo_history::{self, CargoSite};
    let mut stock_draft = state.clone();
    transfer_stock_only(content, &mut stock_draft, side, from, to, amount, packing)?;
    let included = match (from, to) {
        (Endpoint::Pool(pool), Endpoint::Dump(dump)) => Some((pool, dump, false)),
        (Endpoint::Dump(dump), Endpoint::Pool(pool)) => Some((pool, dump, true)),
        _ => None,
    };
    // Validate BOTH existing cargo endpoints even when the first is tagged.
    // Malformed current history must not be hidden by a short-circuit guard.
    let source_tagged = endpoint_has_tags(state, side, from)?;
    let target_tagged = endpoint_has_tags(state, side, to)?;
    let Some((pool, dump, loading)) = included else {
        if matches!((from, to), (Endpoint::Pool(_), Endpoint::Ground(_))) {
            return Err(DistributionError::Cargo(
                DistributionCargoError::UnintegratedRoute(
                    "pool unloading to a new ground dump lacks a cargo-history destination (airlog:53.25)",
                ),
            ));
        }
        if source_tagged || target_tagged {
            return Err(DistributionError::Cargo(
                DistributionCargoError::UnintegratedRoute(
                    "current tagged cargo touches an unintegrated distribution endpoint (airlog:53.25)",
                ),
            ));
        }
        *state = stock_draft;
        return Ok(());
    };
    if state.logistics.dumps[dump].id != *dump {
        return Err(invalid_carrier(
            "distribution dump key disagrees with identity",
        ));
    }
    let timing = pool_timing(content, state, side, pool)?;
    let (source, target, origin, recipient) = if loading {
        (
            CargoSite::Dump(dump.clone()),
            CargoSite::Pool(pool.clone()),
            None,
            Some(timing),
        )
    } else {
        (
            CargoSite::Pool(pool.clone()),
            CargoSite::Dump(dump.clone()),
            Some(timing),
            None,
        )
    };
    let choices = cargo_history::select(state, side, &source, amount, None)?;
    let mut history_draft = state.clone();
    cargo_history::transfer_with_origin(
        &mut history_draft,
        side,
        &source,
        &target,
        amount,
        &choices,
        origin,
        recipient,
    )?;
    stock_draft.logistics.cargo_history = history_draft.logistics.cargo_history;
    *state = stock_draft;
    Ok(())
}

/// Cases: airlog:48.0, airlog:53.24, land:3.6
pub fn enter(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    super::batches::enter_distribution(content, state, cx)
}

pub(super) fn packing_schema(content: &CnaContent, state: &State, to: &Endpoint) -> ActionSchema {
    let attached = match to {
        Endpoint::Cargo(id) => state.land.units[id].trucks,
        Endpoint::Pool(id) => {
            state
                .logistics
                .truck_pools
                .iter()
                .find(|p| &p.id == id)
                .unwrap()
                .trucks
        }
        _ => Trucks::default(),
    };
    ActionSchema::Record {
        fields: [
            ("light", cna_tables::airlog::trucks::TruckType::Light),
            ("medium", cna_tables::airlog::trucks::TruckType::Medium),
            ("heavy", cna_tables::airlog::trucks::TruckType::Heavy),
        ]
        .into_iter()
        .map(|(name, t)| {
            field(
                name,
                "Final cargo packing",
                ActionSchema::Record {
                    fields: TYPES
                        .into_iter()
                        .zip(["ammo", "fuel", "stores", "water"])
                        .map(|(s, name)| {
                            field(
                                name,
                                "Whole supply points",
                                ActionSchema::Integer {
                                    min: 0,
                                    max: i64::from(capacity::trucks(&attached, t))
                                        * i64::from(
                                            content
                                                .tables
                                                .airlog
                                                .truck_characteristics
                                                .truck(t)
                                                .supply_capacity(s),
                                        ),
                                },
                            )
                        })
                        .collect(),
                },
            )
        })
        .collect(),
    }
}
/// Cases: airlog:49.14, airlog:49.16, airlog:53.24, airlog:54.13, land:3.6
/// Interpretations: interp:airlog-0001, interp:airlog-0008
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    if matches!(pending.kind.as_str(), super::batches::DISTRIBUTION) {
        return super::batches::answer(content, state, pending, action, cx, false);
    }
    Err(illegal(
        "single-unit logistics windows are retired; use the fixed batched step",
    ))
}

#[cfg(test)]
mod tests;
