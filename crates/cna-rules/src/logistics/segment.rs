//! Cohort movement fuel. Historical charges are frozen, and a funding account's
//! whole-point source credit is shared once by every cohort split from it.
use super::supply::{
    SupplyDemand, SupplyDraw, SupplyError, SupplySource, available_sources_at_with_content,
    movement_fuel_cost, withdraw_into,
};
use crate::{CnaContent, State, seq::Half, state::Location};
use cna_content::units::Trucks;
use cna_core::{ids::UnitId, quantity::FuelTenths};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentKey {
    pub game_turn: u16,
    pub op_stage: Option<u8>,
    pub half: Option<Half>,
    pub cycle: u16,
}
impl SegmentKey {
    pub(super) fn current(state: &State) -> Self {
        Self {
            game_turn: state.cursor.game_turn,
            op_stage: state.cursor.op_stage,
            half: state.cursor.half,
            cycle: state.cursor.cycle,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuelDraw {
    pub source: SupplySource,
    pub fuel: FuelTenths,
}

/// Physical truck groups keep their identities across movement segments. A split
/// gets a new identity and retains its parent's identity for other physical histories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuelTruckKind {
    Light,
    Medium,
    Heavy,
}
impl FuelTruckKind {
    fn count(self, trucks: &Trucks) -> i32 {
        match self {
            Self::Light => trucks.light,
            Self::Medium => trucks.medium,
            Self::Heavy => trucks.heavy,
        }
    }
}
const KINDS: [FuelTruckKind; 3] = [
    FuelTruckKind::Light,
    FuelTruckKind::Medium,
    FuelTruckKind::Heavy,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruckFuelCohort<A = UnitId> {
    pub id: String,
    pub parent: Option<String>,
    pub kind: FuelTruckKind,
    pub count: i32,
    pub cp_quarters: i32,
    pub account: A,
    pub segment: SegmentKey,
}
/// Charges already funded by one original movement group. Splitting a group never
/// duplicates its source credit. Body and removed-vehicle historical charges stay paid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuelFundingAccount {
    pub side: cna_protocol::Side,
    pub segment: SegmentKey,
    #[serde(deserialize_with = "deserialize_origin")]
    pub origin: Location,
    pub paid_cost: FuelTenths,
    pub draws: Vec<FuelDraw>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(deserialize = "A: Deserialize<'de>"))]
pub struct FuelSegmentLedger<A = UnitId> {
    pub segment: SegmentKey,
    #[serde(deserialize_with = "deserialize_origin")]
    pub origin: Location,
    pub cp_quarters: i32,
    pub paid_cost: FuelTenths,
    pub draws: Vec<FuelDraw>,
    #[serde(default)]
    pub cohorts: Vec<TruckFuelCohort<A>>,
    #[serde(default)]
    pub cohorts_initialized: bool,
    #[serde(default)]
    pub next_cohort_serial: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AccountPlan<A = UnitId> {
    pub(super) id: A,
    pub(super) account: FuelFundingAccount,
    pub(super) prior: BTreeMap<SupplySource, FuelTenths>,
    pub(super) sources: BTreeMap<SupplySource, SupplyDemand>,
    pub(super) draws: Vec<SupplyDraw>,
    pub(super) increment: FuelTenths,
}
/// Pure movement preview. All accounts touched by this preview are committed atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentFuelPlan<A = UnitId> {
    pub ledger: FuelSegmentLedger<A>,
    pub increment: FuelTenths,
    pub draws: Vec<SupplyDraw>,
    pub(super) funding: Vec<AccountPlan<A>>,
}
// Legacy checkpoints stored an origin as a bare hex id. New checkpoints retain
// the actual map or off-map location, including a distinct traveling-group id.
fn deserialize_origin<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Location, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Origin {
        Legacy(cna_core::ids::HexId),
        Current(Location),
    }
    let location = match Origin::deserialize(d)? {
        Origin::Legacy(hex) => Location::Hex { hex },
        Origin::Current(location) => location,
    };
    validate_origin(&location).map_err(|_| serde::de::Error::custom("invalid fuel origin"))?;
    Ok(location)
}
pub(super) fn validate_origin(location: &Location) -> Result<(), SupplyError> {
    match location {
        Location::Hex { .. } => Ok(()),
        Location::OffMap { id } if !id.is_empty() => Ok(()),
        _ => Err(SupplyError::Invalid),
    }
}
fn add(a: i32, b: i32) -> Result<i32, SupplyError> {
    a.checked_add(b).ok_or(SupplyError::Invalid)
}
fn mul(a: i32, b: i32) -> Result<i32, SupplyError> {
    a.checked_mul(b).ok_or(SupplyError::Invalid)
}
pub(super) fn draws_map(
    draws: &[FuelDraw],
    paid: FuelTenths,
) -> Result<BTreeMap<SupplySource, FuelTenths>, SupplyError> {
    if paid.get() < 0 {
        return Err(SupplyError::Invalid);
    }
    let mut result: BTreeMap<SupplySource, FuelTenths> = BTreeMap::new();
    let mut sum = 0;
    for d in draws {
        if d.fuel.get() < 0 || matches!(d.source, SupplySource::ReadyAmmo) {
            return Err(SupplyError::Invalid);
        }
        sum = add(sum, d.fuel.get())?;
        let old = result.get(&d.source).copied().unwrap_or_default().get();
        result.insert(d.source.clone(), FuelTenths::new(add(old, d.fuel.get())?));
    }
    if sum != paid.get() {
        return Err(SupplyError::Invalid);
    }
    Ok(result)
}
fn as_draws(map: BTreeMap<SupplySource, FuelTenths>) -> Vec<FuelDraw> {
    map.into_iter()
        .map(|(source, fuel)| FuelDraw { source, fuel })
        .collect()
}
pub(super) fn credit(source: &SupplySource, previous: FuelTenths) -> i32 {
    if matches!(
        source,
        SupplySource::Tank
            | SupplySource::PoolTank(_)
            | SupplySource::ReadyAmmo
            | SupplySource::Unlimited
    ) {
        0
    } else {
        (10 - previous.get().rem_euclid(10)) % 10
    }
}
pub(super) fn chart(content: &CnaContent, cp_quarters: i32) -> Result<i32, SupplyError> {
    if cp_quarters < 0 {
        return Err(SupplyError::Invalid);
    }
    if cp_quarters == 0 {
        return Ok(0);
    }
    let cp = cp_quarters / 4 + i32::from(cp_quarters % 4 != 0);
    content
        .tables
        .airlog
        .fuel_consumption
        .fuel_for(1, cp)
        .map(|n| n.get())
        .ok_or(SupplyError::Unsupported {
            case: "airlog:49.19",
        })
}
pub(super) fn truck_total(trucks: &Trucks) -> Result<i32, SupplyError> {
    let mut total = 0;
    for k in KINDS {
        let n = k.count(trucks);
        if n < 0 {
            return Err(SupplyError::Invalid);
        }
        total = add(total, n)?
    }
    Ok(total)
}
fn body_cost(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    cp: i32,
) -> Result<i32, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    let full = movement_fuel_cost(content, state, id, cp)?.get();
    full.checked_sub(mul(chart(content, cp)?, truck_total(&unit.trucks)?)?)
        .filter(|v| *v >= 0)
        .ok_or(SupplyError::Invalid)
}
fn new_id<A>(ledger: &mut FuelSegmentLedger<A>, id: &str) -> Result<String, SupplyError> {
    ledger.next_cohort_serial = ledger
        .next_cohort_serial
        .checked_add(1)
        .ok_or(SupplyError::Invalid)?;
    Ok(format!("{id}.fuel-trucks-{}", ledger.next_cohort_serial))
}
/// Canonical identity-only genesis; no origin or funding record is required.
/// Cases: airlog:49.13, land:21.25, land:21.29
pub(super) fn initial_cohorts<A: Clone>(
    trucks: &Trucks,
    account: &A,
    identity: &str,
    segment: &SegmentKey,
    cp_quarters: i32,
    serial: u64,
) -> Result<(Vec<TruckFuelCohort<A>>, u64), SupplyError> {
    if identity.is_empty() || cp_quarters < 0 {
        return Err(SupplyError::Invalid);
    }
    truck_total(trucks)?;
    let mut next = serial;
    let mut cohorts = vec![];
    for kind in KINDS {
        let count = kind.count(trucks);
        if count > 0 {
            next = next.checked_add(1).ok_or(SupplyError::Invalid)?;
            cohorts.push(TruckFuelCohort {
                id: format!("{identity}.fuel-trucks-{next}"),
                parent: None,
                kind,
                count,
                cp_quarters,
                account: account.clone(),
                segment: segment.clone(),
            });
        }
    }
    Ok((cohorts, next))
}
fn validate_cohorts<A>(ledger: &FuelSegmentLedger<A>, trucks: &Trucks) -> Result<(), SupplyError> {
    let mut ids = BTreeSet::new();
    for k in KINDS {
        let count = ledger
            .cohorts
            .iter()
            .filter(|c| c.kind == k)
            .try_fold(0, |n, c| {
                if c.count <= 0
                    || c.cp_quarters < 0
                    || c.segment != ledger.segment
                    || !ids.insert(c.id.clone())
                {
                    return Err(SupplyError::Invalid);
                }
                add(n, c.count)
            })?;
        if count != k.count(trucks) {
            return Err(SupplyError::Invalid);
        }
    }
    Ok(())
}
fn ledger_for(state: &State, id: &UnitId) -> Result<FuelSegmentLedger, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    carrier_ledger(
        state.logistics.fuel_segments.get(id),
        SegmentKey::current(state),
        &unit.location,
        &unit.trucks,
        id,
        id.as_str(),
    )
}

/// Seed or validate physical cohorts without assuming a land-unit identity.
pub(super) fn carrier_ledger<A: Clone>(
    old: Option<&FuelSegmentLedger<A>>,
    segment: SegmentKey,
    location: &Location,
    trucks: &Trucks,
    id: &A,
    identity: &str,
) -> Result<FuelSegmentLedger<A>, SupplyError> {
    let mut ledger = if let Some(old) = old.filter(|l| l.segment == segment) {
        old.clone()
    } else {
        FuelSegmentLedger {
            segment: segment.clone(),
            origin: location.clone(),
            cp_quarters: 0,
            paid_cost: FuelTenths::ZERO,
            draws: vec![],
            cohorts: old.map_or_else(Vec::new, |l| l.cohorts.clone()),
            cohorts_initialized: old.is_some_and(|l| l.cohorts_initialized),
            next_cohort_serial: old.map_or(0, |l| l.next_cohort_serial),
        }
    };
    validate_origin(&ledger.origin)?;
    validate_origin(location)?;
    if ledger.cp_quarters < 0 {
        return Err(SupplyError::Invalid);
    }
    draws_map(&ledger.draws, ledger.paid_cost)?;
    if !ledger.cohorts_initialized {
        let (cohorts, serial) = initial_cohorts(
            trucks,
            id,
            identity,
            &segment,
            ledger.cp_quarters,
            ledger.next_cohort_serial,
        )?;
        ledger.cohorts.extend(cohorts);
        ledger.next_cohort_serial = serial;
        ledger.cohorts_initialized = true;
    } else if old.is_some_and(|l| l.segment != segment) {
        for cohort in &mut ledger.cohorts {
            cohort.cp_quarters = 0;
            cohort.account = id.clone();
            cohort.segment = segment.clone();
        }
    }
    validate_cohorts(&ledger, trucks)?;
    Ok(ledger)
}
fn account_for(
    state: &State,
    id: &UnitId,
    ledger: &FuelSegmentLedger,
) -> Result<FuelFundingAccount, SupplyError> {
    let account = state
        .logistics
        .fuel_accounts
        .get(id)
        .filter(|a| a.segment == ledger.segment)
        .ok_or(SupplyError::Invalid)?;
    validate_origin(&account.origin)?;
    draws_map(&account.draws, account.paid_cost)?;
    Ok(account.clone())
}
fn own_account(
    state: &State,
    id: &UnitId,
    ledger: &FuelSegmentLedger,
) -> Result<FuelFundingAccount, SupplyError> {
    if let Some(a) = state
        .logistics
        .fuel_accounts
        .get(id)
        .filter(|a| a.segment == ledger.segment)
    {
        validate_origin(&a.origin)?;
        draws_map(&a.draws, a.paid_cost)?;
        Ok(a.clone())
    } else {
        Ok(FuelFundingAccount {
            side: state.land.units.get(id).ok_or(SupplyError::Invalid)?.side,
            segment: ledger.segment.clone(),
            origin: ledger.origin.clone(),
            paid_cost: ledger.paid_cost,
            draws: ledger.draws.clone(),
        })
    }
}
fn capacities(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    account: &FuelFundingAccount,
    used: &BTreeMap<SupplySource, i32>,
) -> Result<BTreeMap<SupplySource, SupplyDemand>, SupplyError> {
    if state.land.units.get(id).ok_or(SupplyError::Invalid)?.side != account.side {
        return Err(SupplyError::Invalid);
    }
    let sources: BTreeMap<_, _> =
        available_sources_at_with_content(content, state, id, &account.origin)?
            .into_iter()
            .map(|s| (s.source, s.amount))
            .collect();
    source_capacities(sources, account, used)
}

/// Pure capacity accounting across all accounts touched by one movement preview.
pub(super) fn source_capacities(
    mut sources: BTreeMap<SupplySource, SupplyDemand>,
    account: &FuelFundingAccount,
    used: &BTreeMap<SupplySource, i32>,
) -> Result<BTreeMap<SupplySource, SupplyDemand>, SupplyError> {
    for (source, amount) in &mut sources {
        amount.fuel = FuelTenths::new(
            amount
                .fuel
                .get()
                .checked_sub(used.get(source).copied().unwrap_or(0))
                .filter(|v| *v >= 0)
                .ok_or(SupplyError::Insufficient)?,
        );
    }
    for (source, paid) in draws_map(&account.draws, account.paid_cost)? {
        let amount = sources.entry(source.clone()).or_default();
        amount.fuel = FuelTenths::new(add(amount.fuel.get(), credit(&source, paid))?);
    }
    Ok(sources)
}
/// Price only new physical movement. Changing composition freezes earlier charges;
/// incoming truck cohorts continue their own CP history, independently of body CP.
/// Origins retain their actual location. An off-map traveling group may draw from
/// carried first-line stocks at that exact group location; it gains no access to
/// a departed box or unrelated traveling groups. Each new segment captures the
/// current location, preserving physical cohorts and their funding credit.
/// No source scan is needed for a zero increment.
/// Cases: airlog:49.12, airlog:49.13, airlog:49.15, airlog:49.16, land:8.83, land:8.84
/// Interpretations: interp:airlog-0001, interp:airlog-0018
pub fn plan_segment_fuel(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    total_cp_quarters: i32,
) -> Result<SegmentFuelPlan, SupplyError> {
    let mut ledger = ledger_for(state, id)?;
    let before_body = body_cost(content, state, id, ledger.cp_quarters)?;
    let after_body = body_cost(content, state, id, total_cp_quarters)?;
    let body_increment = after_body
        .checked_sub(before_body)
        .filter(|n| *n >= 0)
        .ok_or(SupplyError::Invalid)?;
    let costs = cohort_costs(content, &mut ledger, id, total_cp_quarters, body_increment)?;
    fund_plan(
        ledger,
        total_cp_quarters,
        costs,
        |account_id, ledger| {
            if account_id == id {
                own_account(state, id, ledger)
            } else {
                account_for(state, account_id, ledger)
            }
        },
        |account, used| capacities(content, state, id, account, used),
    )
}

/// Pure cohort pricing: the body obligation is supplied separately, never invented for pools.
pub(super) fn cohort_costs<A: Clone + Ord>(
    content: &CnaContent,
    ledger: &mut FuelSegmentLedger<A>,
    id: &A,
    total_cp_quarters: i32,
    body_increment: i32,
) -> Result<BTreeMap<A, i32>, SupplyError> {
    let delta = total_cp_quarters
        .checked_sub(ledger.cp_quarters)
        .filter(|n| *n >= 0)
        .ok_or(SupplyError::Invalid)?;
    let mut costs: BTreeMap<A, i32> = BTreeMap::new();
    if body_increment > 0 {
        costs.insert(id.clone(), body_increment);
    }
    for cohort in &mut ledger.cohorts {
        let next = add(cohort.cp_quarters, delta)?;
        let increase = mul(
            chart(content, next)? - chart(content, cohort.cp_quarters)?,
            cohort.count,
        )?;
        if increase < 0 {
            return Err(SupplyError::Invalid);
        }
        let old = costs.get(&cohort.account).copied().unwrap_or(0);
        if increase > 0 {
            costs.insert(cohort.account.clone(), add(old, increase)?);
        }
        cohort.cp_quarters = next;
    }
    Ok(costs)
}

/// Shared account funding, retaining each original source's rounding credit exactly once.
pub(super) fn fund_plan<A: Clone + Ord>(
    mut ledger: FuelSegmentLedger<A>,
    total_cp_quarters: i32,
    costs: BTreeMap<A, i32>,
    mut get_account: impl FnMut(&A, &FuelSegmentLedger<A>) -> Result<FuelFundingAccount, SupplyError>,
    mut get_capacities: impl FnMut(
        &FuelFundingAccount,
        &BTreeMap<SupplySource, i32>,
    ) -> Result<BTreeMap<SupplySource, SupplyDemand>, SupplyError>,
) -> Result<SegmentFuelPlan<A>, SupplyError> {
    let increment = FuelTenths::new(costs.values().try_fold(0, |n, v| add(n, *v))?);
    let mut combined = draws_map(&ledger.draws, ledger.paid_cost)?;
    let mut funding = Vec::new();
    let mut all_draws = Vec::new();
    let mut used = BTreeMap::new();
    for (account_id, cost) in costs {
        let mut account = get_account(&account_id, &ledger)?;
        let prior = draws_map(&account.draws, account.paid_cost)?;
        let sources = get_capacities(&account, &used)?;
        let mut choices: Vec<_> = sources.iter().collect();
        choices.sort_by_key(|(s, _)| {
            (
                credit(s, prior.get(s).copied().unwrap_or_default()) == 0,
                (*s).clone(),
            )
        });
        let mut need = cost;
        let mut draws = Vec::new();
        let mut new_prior = prior.clone();
        for (source, amount) in choices {
            let take = amount.fuel.get().min(need);
            if take == 0 {
                continue;
            }
            need -= take;
            let before = prior.get(source).copied().unwrap_or_default();
            let after = FuelTenths::new(add(before.get(), take)?);
            let actual = match source {
                SupplySource::Unlimited => 0,
                SupplySource::Tank | SupplySource::PoolTank(_) => take,
                _ => mul((after.ceil_points() - before.ceil_points()).get(), 10)?,
            };
            let old = used.get(source).copied().unwrap_or(0);
            used.insert(source.clone(), add(old, actual)?);
            new_prior.insert(source.clone(), after);
            let old = combined.get(source).copied().unwrap_or_default();
            combined.insert(source.clone(), FuelTenths::new(add(old.get(), take)?));
            draws.push(SupplyDraw {
                source: source.clone(),
                amount: SupplyDemand {
                    fuel: FuelTenths::new(take),
                    ..SupplyDemand::default()
                },
            });
        }
        if need != 0 {
            return Err(SupplyError::Insufficient);
        }
        account.paid_cost = FuelTenths::new(add(account.paid_cost.get(), cost)?);
        account.draws = as_draws(new_prior);
        all_draws.extend(draws.clone());
        funding.push(AccountPlan {
            id: account_id,
            account,
            prior,
            sources,
            draws,
            increment: FuelTenths::new(cost),
        });
    }
    ledger.cp_quarters = total_cp_quarters;
    ledger.paid_cost = FuelTenths::new(add(ledger.paid_cost.get(), increment.get())?);
    ledger.draws = as_draws(combined);
    Ok(SegmentFuelPlan {
        ledger,
        increment,
        draws: all_draws,
        funding,
    })
}
/// Atomically commit every physical cohort and funding withdrawal in the preview.
/// Cases: airlog:49.13, airlog:49.15, airlog:49.16
/// Interpretations: interp:airlog-0001, interp:airlog-0018
pub fn spend_segment_fuel(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    total_cp_quarters: i32,
) -> Result<Vec<SupplyDraw>, SupplyError> {
    Ok(spend_segment_fuel_report(content, state, id, total_cp_quarters)?.draws)
}

/// The incremental cost and actual draws from a single validated atomic spend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentFuelSpent {
    pub increment: FuelTenths,
    pub draws: Vec<SupplyDraw>,
}

/// Plan once and atomically commit all cohort funding, returning its exact increment.
/// Cases: airlog:49.13, airlog:49.15, airlog:49.16
/// Interpretations: interp:airlog-0001, interp:airlog-0018
pub fn spend_segment_fuel_report(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    total_cp_quarters: i32,
) -> Result<SegmentFuelSpent, SupplyError> {
    let plan = plan_segment_fuel(content, state, id, total_cp_quarters)?;
    if plan.increment.is_zero() {
        state
            .logistics
            .fuel_segments
            .insert(id.clone(), plan.ledger);
        return Ok(SegmentFuelSpent {
            increment: plan.increment,
            draws: plan.draws,
        });
    }
    let mut next = state.logistics.clone();
    for funding in &plan.funding {
        withdraw_into(
            &mut next,
            Some(id),
            SupplyDemand {
                fuel: funding.increment,
                ..SupplyDemand::default()
            },
            &funding.draws,
            &funding.sources,
            &funding.prior,
        )?;
        next.fuel_accounts
            .insert(funding.id.clone(), funding.account.clone());
    }
    next.fuel_segments.insert(id.clone(), plan.ledger);
    state.logistics = next;
    Ok(SegmentFuelSpent {
        increment: plan.increment,
        draws: plan.draws,
    })
}
/// Snapshot just the shared accounts a movement plan can change. Capture before
/// hypothetical execution, restore afterwards along with unit ledgers and holdings.
#[derive(Debug, Clone)]
pub struct FuelAccountSnapshot {
    accounts: BTreeMap<UnitId, Option<FuelFundingAccount>>,
}
pub fn snapshot_fuel_accounts(state: &State, units: &[UnitId]) -> FuelAccountSnapshot {
    let mut keys: BTreeSet<_> = units.iter().cloned().collect();
    for id in units {
        if let Some(ledger) = state.logistics.fuel_segments.get(id) {
            keys.extend(ledger.cohorts.iter().map(|c| c.account.clone()));
        }
    }
    FuelAccountSnapshot {
        accounts: keys
            .into_iter()
            .map(|id| {
                let old = state.logistics.fuel_accounts.get(&id).cloned();
                (id, old)
            })
            .collect(),
    }
}
pub fn restore_fuel_accounts(state: &mut State, snapshot: &FuelAccountSnapshot) {
    for (id, old) in &snapshot.accounts {
        if let Some(old) = old {
            state
                .logistics
                .fuel_accounts
                .insert(id.clone(), old.clone());
        } else {
            state.logistics.fuel_accounts.remove(id);
        }
    }
}
fn initialize(
    state: &State,
    id: &UnitId,
) -> Result<(FuelSegmentLedger, FuelFundingAccount), SupplyError> {
    let ledger = ledger_for(state, id)?;
    let account = own_account(state, id, &ledger)?;
    Ok((ledger, account))
}
fn select(
    ledger: &mut FuelSegmentLedger,
    owner: &UnitId,
    trucks: Trucks,
) -> Result<Vec<TruckFuelCohort>, SupplyError> {
    let mut selected = Vec::new();
    for kind in KINDS {
        let mut need = kind.count(&trucks);
        if need < 0 {
            return Err(SupplyError::Invalid);
        }
        let length = ledger.cohorts.len();
        for i in 0..length {
            if need == 0 {
                break;
            }
            if ledger.cohorts[i].kind != kind {
                continue;
            }
            let take = need.min(ledger.cohorts[i].count);
            let mut cohort = ledger.cohorts[i].clone();
            if take < cohort.count {
                let parent = cohort.id.clone();
                cohort.id = new_id(ledger, owner.as_str())?;
                cohort.parent = Some(parent);
            }
            ledger.cohorts[i].count -= take;
            cohort.count = take;
            need -= take;
            selected.push(cohort);
        }
        if need != 0 {
            return Err(SupplyError::Invalid);
        }
    }
    ledger.cohorts.retain(|c| c.count > 0);
    Ok(selected)
}
/// Before physical truck-count mutation, transfer cohorts at a friendly shared hex.
/// The returned split lineage lets Land transfer truck BP history without body BP.
/// Cargo, tanks, water credit, and physical counts are separate explicit operations.
/// Cases: land:8.56, airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0018
pub fn transfer_segment_fuel_cohorts(
    state: &mut State,
    from: &UnitId,
    to: &UnitId,
    trucks: Trucks,
) -> Result<Vec<TruckFuelCohort>, SupplyError> {
    if from == to {
        return Err(SupplyError::Invalid);
    }
    let a = state.land.units.get(from).ok_or(SupplyError::Invalid)?;
    let b = state.land.units.get(to).ok_or(SupplyError::Invalid)?;
    if a.side != b.side || a.location != b.location || a.location.hex().is_none() {
        return Err(SupplyError::Invalid);
    }
    let (mut donor, da) = initialize(state, from)?;
    let (mut recipient, ra) = initialize(state, to)?;
    let cohorts = select(&mut donor, from, trucks)?;
    let mut known: BTreeSet<_> = recipient.cohorts.iter().map(|c| c.id.clone()).collect();
    for c in &cohorts {
        if !known.insert(c.id.clone()) {
            return Err(SupplyError::Invalid);
        }
    }
    recipient.cohorts.extend(cohorts.clone());
    state.logistics.fuel_accounts.insert(from.clone(), da);
    state.logistics.fuel_accounts.insert(to.clone(), ra);
    state.logistics.fuel_segments.insert(from.clone(), donor);
    state.logistics.fuel_segments.insert(to.clone(), recipient);
    Ok(cohorts)
}
/// Before removing broken or destroyed trucks, retain their exact moving history.
/// Store the returned cohorts with broken trucks; destroyed cohorts may be discarded.
/// The funding account retains their already-paid contribution in either case.
/// Cases: land:21.25, land:21.29, airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0018
pub fn remove_segment_fuel_cohorts(
    state: &mut State,
    id: &UnitId,
    trucks: Trucks,
) -> Result<Vec<TruckFuelCohort>, SupplyError> {
    let (mut ledger, account) = initialize(state, id)?;
    let cohorts = select(&mut ledger, id, trucks)?;
    state.logistics.fuel_accounts.insert(id.clone(), account);
    state.logistics.fuel_segments.insert(id.clone(), ledger);
    Ok(cohorts)
}
/// Before adding recovered trucks, restore their physical history. Later segments
/// begin a new fuel charge while stable identities remain available for BP history.
/// Cases: land:21.25, land:21.29, airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0018
pub fn restore_segment_fuel_cohorts(
    state: &mut State,
    id: &UnitId,
    cohorts: &[TruckFuelCohort],
) -> Result<(), SupplyError> {
    let (mut ledger, account) = initialize(state, id)?;
    let owner_side = state.land.units.get(id).ok_or(SupplyError::Invalid)?.side;
    let mut ids: BTreeSet<_> = state
        .logistics
        .fuel_segments
        .iter()
        .filter(|(owner, _)| {
            *owner != id
                && state
                    .land
                    .units
                    .get(*owner)
                    .is_some_and(|u| u.side == owner_side)
        })
        .flat_map(|(_, l)| l.cohorts.iter().map(|c| c.id.clone()))
        .chain(ledger.cohorts.iter().map(|c| c.id.clone()))
        .collect();
    for original in cohorts {
        if original.count <= 0 || original.cp_quarters < 0 || !ids.insert(original.id.clone()) {
            return Err(SupplyError::Invalid);
        }
        let mut c = original.clone();
        if c.segment != ledger.segment {
            c.segment = ledger.segment.clone();
            c.cp_quarters = 0;
            c.account = id.clone();
        } else if !state
            .logistics
            .fuel_accounts
            .get(&c.account)
            .is_some_and(|a| a.segment == ledger.segment && a.side == owner_side)
        {
            return Err(SupplyError::Invalid);
        }
        ledger.cohorts.push(c);
    }
    state.logistics.fuel_accounts.insert(id.clone(), account);
    state.logistics.fuel_segments.insert(id.clone(), ledger);
    Ok(())
}

/// Select exact physical groups, as needed when trucks have different breakdown histories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FuelCohortSelection {
    pub id: String,
    pub count: i32,
}

/// Current physical cohorts, without mutating state or discovering other units.
/// Cases: airlog:49.13, land:21.25, land:21.29
pub fn segment_fuel_cohorts(
    state: &State,
    id: &UnitId,
) -> Result<Vec<TruckFuelCohort>, SupplyError> {
    Ok(ledger_for(state, id)?.cohorts)
}
pub(super) fn select_named<A: Clone>(
    ledger: &mut FuelSegmentLedger<A>,
    owner: &str,
    selection: &[FuelCohortSelection],
) -> Result<Vec<TruckFuelCohort<A>>, SupplyError> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for choice in selection {
        if choice.count <= 0 || !seen.insert(choice.id.clone()) {
            return Err(SupplyError::Invalid);
        }
        let i = ledger
            .cohorts
            .iter()
            .position(|c| c.id == choice.id)
            .ok_or(SupplyError::Invalid)?;
        let mut cohort = ledger.cohorts[i].clone();
        if choice.count > cohort.count {
            return Err(SupplyError::Invalid);
        }
        if choice.count < cohort.count {
            let parent = cohort.id.clone();
            cohort.id = new_id(ledger, owner)?;
            cohort.parent = Some(parent);
        }
        cohort.count = choice.count;
        ledger.cohorts[i].count -= choice.count;
        result.push(cohort);
    }
    ledger.cohorts.retain(|c| c.count > 0);
    Ok(result)
}
/// Transfer only the chosen physical groups, before changing attached truck counts.
/// Cases: land:8.56, airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0018
pub fn transfer_selected_segment_fuel_cohorts(
    state: &mut State,
    from: &UnitId,
    to: &UnitId,
    selection: &[FuelCohortSelection],
) -> Result<Vec<TruckFuelCohort>, SupplyError> {
    if from == to {
        return Err(SupplyError::Invalid);
    }
    let a = state.land.units.get(from).ok_or(SupplyError::Invalid)?;
    let b = state.land.units.get(to).ok_or(SupplyError::Invalid)?;
    if a.side != b.side || a.location != b.location || a.location.hex().is_none() {
        return Err(SupplyError::Invalid);
    }
    let (mut donor, da) = initialize(state, from)?;
    let (mut recipient, ra) = initialize(state, to)?;
    let cohorts = select_named(&mut donor, from.as_str(), selection)?;
    let mut known: BTreeSet<_> = recipient.cohorts.iter().map(|c| c.id.clone()).collect();
    for c in &cohorts {
        if !known.insert(c.id.clone()) {
            return Err(SupplyError::Invalid);
        }
    }
    recipient.cohorts.extend(cohorts.clone());
    state.logistics.fuel_accounts.insert(from.clone(), da);
    state.logistics.fuel_accounts.insert(to.clone(), ra);
    state.logistics.fuel_segments.insert(from.clone(), donor);
    state.logistics.fuel_segments.insert(to.clone(), recipient);
    Ok(cohorts)
}
/// Remove only the groups that suffered the rolled loss; returned history travels
/// with a broken-truck marker and is not replaced by its unit body's history.
/// Cases: land:21.25, land:21.29, airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0018
pub fn remove_selected_segment_fuel_cohorts(
    state: &mut State,
    id: &UnitId,
    selection: &[FuelCohortSelection],
) -> Result<Vec<TruckFuelCohort>, SupplyError> {
    let (mut ledger, account) = initialize(state, id)?;
    let cohorts = select_named(&mut ledger, id.as_str(), selection)?;
    state.logistics.fuel_accounts.insert(id.clone(), account);
    state.logistics.fuel_segments.insert(id.clone(), ledger);
    Ok(cohorts)
}

#[cfg(test)]
mod tests;
