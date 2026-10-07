//! Per-load CP history prevents unloading and reloading from resetting travel.
//! The owner chooses lots only when their movement histories differ.
use super::{SupplyError, water::WaterStage};
use crate::{State, state::LogisticsState};
use cna_content::scenario::Supplies;
use cna_core::engine::Rejection;
use cna_core::{ids::UnitId, visibility::Perspective};
use cna_protocol::Side;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum CargoSite {
    Unit(UnitId),
    Pool(String),
    Dump(String),
    Ship(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CargoError {
    Invalid,
    Insufficient,
    Ceiling,
}
impl From<CargoError> for SupplyError {
    fn from(e: CargoError) -> Self {
        match e {
            CargoError::Insufficient => Self::Insufficient,
            _ => Self::Invalid,
        }
    }
}
pub fn rejection(e: CargoError) -> Rejection {
    crate::steps::illegal(match e {
        CargoError::Ceiling => "goods exceed their first carrier's CP ceiling (airlog:53.25)",
        CargoError::Insufficient => "cargo parcel quantity is unavailable",
        CargoError::Invalid => "invalid cargo history or parcel choice",
    })
}
impl CargoSite {
    fn carrier(&self) -> bool {
        matches!(self, Self::Unit(_) | Self::Pool(_))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoLot {
    pub id: String,
    pub goods: Supplies,
    pub spent_cp_quarters: i32,
    pub ceiling_cp_quarters: i32,
    #[serde(default)]
    pub continuous_first_line: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoHistory {
    pub stage: WaterStage,
    pub lots: Vec<CargoLot>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CargoHistoryState {
    #[serde(with = "map_entries")]
    pub histories: BTreeMap<CargoSite, CargoHistory>,
    pub next_id: BTreeMap<Side, u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoEntry {
    pub site: CargoSite,
    pub history: CargoHistory,
}
mod map_entries {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        map: &BTreeMap<CargoSite, CargoHistory>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        map.iter()
            .map(|(site, history)| CargoEntry {
                site: site.clone(),
                history: history.clone(),
            })
            .collect::<Vec<_>>()
            .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<CargoSite, CargoHistory>, D::Error> {
        let entries = Vec::<CargoEntry>::deserialize(d)?;
        let mut map = BTreeMap::new();
        for entry in entries {
            if map.insert(entry.site, entry.history).is_some() {
                return Err(serde::de::Error::custom("duplicate cargo history site"));
            }
        }
        Ok(map)
    }
}
#[derive(Debug, Clone, Copy)]
pub struct CarrierTiming {
    pub spent_cp_quarters: i32,
    pub cpa_quarters: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LotSelection {
    pub lot: String,
    pub goods: Supplies,
}
/// Untagged stock has no earlier carrier. Its public-to-owner choice is "fresh".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parcel {
    pub lot: String,
    pub goods: Supplies,
    pub spent_cp_quarters: i32,
    pub ceiling_cp_quarters: Option<i32>,
}
fn get(s: &Supplies, k: usize) -> i32 {
    [s.ammo, s.fuel, s.stores, s.water][k]
}
fn set(s: &mut Supplies, k: usize, n: i32) {
    match k {
        0 => s.ammo = n,
        1 => s.fuel = n,
        2 => s.stores = n,
        3 => s.water = n,
        _ => unreachable!(),
    }
}
fn valid(s: &Supplies) -> bool {
    (0..4).all(|k| get(s, k) >= 0)
}
fn empty(s: &Supplies) -> bool {
    (0..4).all(|k| get(s, k) == 0)
}
fn add(a: &mut Supplies, b: &Supplies) -> Result<(), CargoError> {
    if !valid(b) {
        return Err(CargoError::Invalid);
    }
    for k in 0..4 {
        set(
            a,
            k,
            get(a, k)
                .checked_add(get(b, k))
                .ok_or(CargoError::Invalid)?,
        );
    }
    Ok(())
}
fn sub(a: &mut Supplies, b: &Supplies) -> Result<(), CargoError> {
    if !valid(b) {
        return Err(CargoError::Invalid);
    }
    for k in 0..4 {
        set(
            a,
            k,
            get(a, k)
                .checked_sub(get(b, k))
                .filter(|n| *n >= 0)
                .ok_or(CargoError::Insufficient)?,
        );
    }
    Ok(())
}
pub fn owner(state: &State, site: &CargoSite) -> Option<Side> {
    match site {
        CargoSite::Unit(id) => state.land.units.get(id).map(|u| u.side),
        CargoSite::Pool(id) => state
            .logistics
            .truck_pools
            .iter()
            .find(|p| &p.id == id)
            .map(|p| p.side),
        CargoSite::Dump(id) => state.logistics.dumps.get(id).map(|d| d.side),
        CargoSite::Ship(id) => state
            .logistics
            .coastal_ships
            .contains_key(id)
            .then_some(Side::Axis),
    }
}
fn stock(state: &State, site: &CargoSite) -> Result<Supplies, CargoError> {
    if owner(state, site).is_none() {
        return Err(CargoError::Invalid);
    }
    let s = match site {
        CargoSite::Unit(id) => state
            .logistics
            .unit_supply
            .get(id)
            .map_or(Supplies::default(), |s| s.carried),
        CargoSite::Pool(id) => {
            state
                .logistics
                .truck_pools
                .iter()
                .find(|p| &p.id == id)
                .unwrap()
                .cargo
        }
        CargoSite::Dump(id) => state.logistics.dumps[id].supplies,
        CargoSite::Ship(id) => state.logistics.coastal_ships[id].cargo,
    };
    if !valid(&s) {
        return Err(CargoError::Invalid);
    }
    Ok(s)
}
fn validate(h: &CargoHistory) -> Result<(), CargoError> {
    let mut ids = BTreeSet::new();
    for lot in &h.lots {
        if lot.id.is_empty()
            || lot.id == "fresh"
            || !ids.insert(&lot.id)
            || !valid(&lot.goods)
            || lot.spent_cp_quarters < 0
            || lot.ceiling_cp_quarters <= 0
        {
            return Err(CargoError::Invalid);
        }
    }
    Ok(())
}
fn totals(h: &CargoHistory) -> Result<Supplies, CargoError> {
    let mut n = Supplies::default();
    for lot in &h.lots {
        add(&mut n, &lot.goods)?;
    }
    Ok(n)
}
fn retire(h: &mut CargoHistory, amount: &Supplies) -> Result<(), CargoError> {
    validate(h)?;
    if !valid(amount) {
        return Err(CargoError::Invalid);
    }
    h.lots.sort_by(|a, b| {
        b.spent_cp_quarters
            .cmp(&a.spent_cp_quarters)
            .then_with(|| a.ceiling_cp_quarters.cmp(&b.ceiling_cp_quarters))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut left = *amount;
    for lot in &mut h.lots {
        for k in 0..4 {
            let n = get(&left, k).min(get(&lot.goods, k));
            let before = get(&left, k);
            set(&mut left, k, before - n);
            let before = get(&lot.goods, k);
            set(&mut lot.goods, k, before - n);
        }
    }
    h.lots.retain(|l| !empty(&l.goods));
    h.lots.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(())
}
fn normalized(state: &State, site: &CargoSite) -> Result<(CargoHistory, Supplies), CargoError> {
    let stage = WaterStage::current(state);
    let mut h = state
        .logistics
        .cargo_history
        .histories
        .get(site)
        .filter(|h| h.stage == stage)
        .cloned()
        .unwrap_or(CargoHistory {
            stage,
            lots: vec![],
        });
    validate(&h)?;
    let stock = stock(state, site)?;
    let total = totals(&h)?;
    let mut deficit = Supplies::default();
    for k in 0..4 {
        set(&mut deficit, k, (get(&total, k) - get(&stock, k)).max(0));
    }
    // Defensive reconciliation covers legacy/direct inventory reductions. New
    // stock writers retire tags at the debit, before a fresh addition can mask it.
    retire(&mut h, &deficit)?;
    let mut fresh = stock;
    sub(&mut fresh, &totals(&h)?)?;
    Ok((h, fresh))
}
/// Owner-known lots only; expiry is by stage query without a global reset.
/// Cases: airlog:53.24, airlog:53.25, land:3.6
/// Interpretations: interp:airlog-0020
pub fn parcels(state: &State, side: Side, site: &CargoSite) -> Result<Vec<Parcel>, CargoError> {
    if owner(state, site) != Some(side) {
        return Err(CargoError::Invalid);
    }
    let (h, fresh) = normalized(state, site)?;
    let mut parcels = h
        .lots
        .into_iter()
        .map(|l| Parcel {
            lot: l.id,
            goods: l.goods,
            spent_cp_quarters: l.spent_cp_quarters,
            ceiling_cp_quarters: Some(l.ceiling_cp_quarters),
        })
        .collect::<Vec<_>>();
    if !empty(&fresh) {
        parcels.push(Parcel {
            lot: "fresh".into(),
            goods: fresh,
            spent_cp_quarters: 0,
            ceiling_cp_quarters: None,
        });
    }
    Ok(parcels)
}
fn next_id(history: &mut CargoHistoryState, side: Side) -> Result<String, CargoError> {
    let n = history.next_id.entry(side).or_default();
    *n = n.checked_add(1).ok_or(CargoError::Invalid)?;
    Ok(format!("{}.cargo-{n}", crate::state::side_key(side)))
}
fn timing(t: CarrierTiming) -> Result<(), CargoError> {
    if t.spent_cp_quarters < 0 || t.cpa_quarters <= 0 {
        Err(CargoError::Invalid)
    } else {
        Ok(())
    }
}
/// Charge goods the carrier's CP, including handling. Caller supplies trusted
/// physical CP/CPA, and commits inventory/CP on the same transactional draft.
/// Cases: airlog:53.24, airlog:53.25
/// Interpretations: interp:airlog-0020
pub fn advance(
    state: &mut State,
    side: Side,
    site: &CargoSite,
    previous: CarrierTiming,
    delta: i32,
) -> Result<(), CargoError> {
    if !site.carrier() || owner(state, site) != Some(side) || delta < 0 {
        return Err(CargoError::Invalid);
    }
    timing(previous)?;
    let (mut h, fresh) = normalized(state, site)?;
    let mut history = state.logistics.cargo_history.clone();
    if !empty(&fresh) && (delta > 0 || previous.spent_cp_quarters > 0) {
        h.lots.push(CargoLot {
            id: next_id(&mut history, side)?,
            goods: fresh,
            spent_cp_quarters: previous.spent_cp_quarters,
            ceiling_cp_quarters: previous.cpa_quarters,
            continuous_first_line: matches!(site, CargoSite::Unit(_)),
        });
    }
    for lot in &mut h.lots {
        lot.spent_cp_quarters = lot
            .spent_cp_quarters
            .max(previous.spent_cp_quarters)
            .checked_add(delta)
            .ok_or(CargoError::Invalid)?;
        if lot.spent_cp_quarters > lot.ceiling_cp_quarters
            && !(matches!(site, CargoSite::Unit(_)) && lot.continuous_first_line)
        {
            return Err(CargoError::Ceiling);
        }
    }
    h.lots.sort_by(|a, b| a.id.cmp(&b.id));
    history.histories.insert(site.clone(), h);
    state.logistics.cargo_history = history;
    Ok(())
}
/// Move exactly selected parcels before the matching stock transfer. A partial
/// parcel gets a new owner-local identity; its CP and first allowance persist.
/// Cases: airlog:53.24, airlog:53.25
/// Interpretations: interp:airlog-0020
#[allow(clippy::too_many_arguments)]
pub fn transfer(
    state: &mut State,
    side: Side,
    from: &CargoSite,
    to: &CargoSite,
    amount: Supplies,
    selection: &[LotSelection],
    recipient: Option<CarrierTiming>,
) -> Result<(), CargoError> {
    if from == to
        || owner(state, from) != Some(side)
        || owner(state, to) != Some(side)
        || !valid(&amount)
        || empty(&amount)
        || to.carrier() != recipient.is_some()
    {
        return Err(CargoError::Invalid);
    }
    if let Some(t) = recipient {
        timing(t)?;
    }
    let (mut source, mut fresh) = normalized(state, from)?;
    let (mut dest, _) = normalized(state, to)?;
    let mut history = state.logistics.cargo_history.clone();
    let mut total = Supplies::default();
    let mut seen = BTreeSet::new();
    for choice in selection {
        if !seen.insert(&choice.lot) || !valid(&choice.goods) || empty(&choice.goods) {
            return Err(CargoError::Invalid);
        }
        add(&mut total, &choice.goods)?;
        let mut carried = if choice.lot == "fresh" {
            sub(&mut fresh, &choice.goods)?;
            recipient.map(|t| CargoLot {
                id: String::new(),
                goods: choice.goods,
                spent_cp_quarters: t.spent_cp_quarters,
                ceiling_cp_quarters: t.cpa_quarters,
                continuous_first_line: matches!(to, CargoSite::Unit(_)),
            })
        } else {
            let lot = source
                .lots
                .iter_mut()
                .find(|l| l.id == choice.lot)
                .ok_or(CargoError::Invalid)?;
            let mut carried = lot.clone();
            sub(&mut lot.goods, &choice.goods)?;
            carried.continuous_first_line = false;
            carried.goods = choice.goods;
            if !empty(&lot.goods) {
                carried.id = String::new();
            }
            Some(carried)
        };
        if let Some(lot) = &mut carried {
            if let Some(t) = recipient {
                lot.spent_cp_quarters = lot.spent_cp_quarters.max(t.spent_cp_quarters);
            }
            if recipient.is_some() && lot.spent_cp_quarters > lot.ceiling_cp_quarters {
                return Err(CargoError::Ceiling);
            }
            if lot.id.is_empty() {
                lot.id = next_id(&mut history, side)?;
            }
        }
        if let Some(lot) = carried {
            dest.lots.push(lot);
        }
    }
    if total != amount {
        return Err(CargoError::Invalid);
    }
    source.lots.retain(|l| !empty(&l.goods));
    dest.lots.sort_by(|a, b| a.id.cmp(&b.id));
    history.histories.insert(from.clone(), source);
    history.histories.insert(to.clone(), dest);
    state.logistics.cargo_history = history;
    Ok(())
}
/// Retire tagged stocks before an automatic debit. Tagged parcels are consumed
/// before untagged stocks, greatest used CP first; ties use lower CPA then id.
/// Cases: airlog:53.25
/// Interpretations: interp:airlog-0020
pub fn retire_debit(
    logistics: &mut LogisticsState,
    site: &CargoSite,
    amount: Supplies,
) -> Result<(), SupplyError> {
    if let Some(h) = logistics.cargo_history.histories.get_mut(site) {
        retire(h, &amount).map_err(SupplyError::from)?;
    }
    Ok(())
}
pub fn disclosed(state: &State, perspective: Perspective) -> Vec<CargoEntry> {
    state
        .logistics
        .cargo_history
        .histories
        .keys()
        .filter(|site| {
            owner(state, site).is_some_and(|side| crate::view::sees_side(perspective, side))
        })
        .filter_map(|site| {
            normalized(state, site).ok().map(|(history, _)| CargoEntry {
                site: site.clone(),
                history,
            })
        })
        .filter(|entry| !entry.history.lots.is_empty())
        .collect()
}

#[derive(Debug, Clone)]
pub struct CargoSnapshot {
    entries: BTreeMap<CargoSite, Option<CargoHistory>>,
    serials: BTreeMap<Side, Option<u64>>,
}
/// Capture only sites a hypothetical operation may change, including its supply
/// sources. Restore the side-local serial too so inspection allocates no identities.
pub fn snapshot(state: &State, sites: &[CargoSite]) -> CargoSnapshot {
    CargoSnapshot {
        entries: sites
            .iter()
            .map(|site| {
                (
                    site.clone(),
                    state.logistics.cargo_history.histories.get(site).cloned(),
                )
            })
            .collect(),
        serials: sites
            .iter()
            .filter_map(|site| owner(state, site))
            .map(|side| {
                (
                    side,
                    state.logistics.cargo_history.next_id.get(&side).copied(),
                )
            })
            .collect(),
    }
}
pub fn restore(state: &mut State, snapshot: &CargoSnapshot) {
    for (site, h) in &snapshot.entries {
        if let Some(h) = h {
            state
                .logistics
                .cargo_history
                .histories
                .insert(site.clone(), h.clone());
        } else {
            state.logistics.cargo_history.histories.remove(site);
        }
    }
    for (side, n) in &snapshot.serials {
        if let Some(n) = n {
            state.logistics.cargo_history.next_id.insert(*side, *n);
        } else {
            state.logistics.cargo_history.next_id.remove(side);
        }
    }
}

#[cfg(test)]
mod tests;
