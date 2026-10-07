//! Anonymous barrage targets and simultaneous private firing plans.
use super::{Position, available};
use crate::{
    CnaContent, State, logistics, ownership,
    state::Pending,
    steps::{illegal, open},
};
use cna_content::units::Toe;
use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    dice::CampaignRng,
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{HexId, SeatId, UnitId},
    quantity::{AmmoPoints, ToeStrengthPoints},
    visibility::{Audience, Perspective},
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::{
    airlog::supply::{AmmoAction, AmmoMode},
    land::{barrage::BarrageTarget, combat::StrengthActivity, terrain::CombatShift},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub const DECLARE: &str = "cna.combat.barrage_hexes";
pub const PLOT: &str = "cna.combat.barrage";
pub const LOSSES: &str = "cna.combat.barrage_losses";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub label: String,
    pub hex: HexId,
    pub class: BarrageTarget,
    pub unit: UnitId,
    pub weapons: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draw {
    pub source: String,
    pub ammo: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub unit: UnitId,
    pub weapon: Option<String>,
    pub toe: i32,
    pub draws: Vec<Draw>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fire {
    pub target: String,
    pub guns: Vec<Contribution>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BarrageState {
    #[serde(default)]
    pub strict: bool,
    pub generation: u64,
    pub declarations: BTreeMap<SeatId, Vec<HexId>>,
    pub targets: BTreeMap<Side, Vec<Target>>,
    pub plans: BTreeMap<SeatId, Vec<Fire>>,
    pub casualties: BTreeMap<UnitId, Vec<Casualty>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Casualty {
    pub weapons: Vec<String>,
    pub loss: i32,
    pub pinned: bool,
    pub trucks: i32,
}
fn seats() -> impl Iterator<Item = SeatId> {
    Side::ALL.into_iter().flat_map(|side| {
        [Role::FrontLine, Role::RearArea, Role::Logistics].map(|role| SeatId::new(side, role))
    })
}
fn field(name: &str, schema: ActionSchema, optional: bool) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: name.into(),
        schema,
        optional,
    }
}
fn options(items: impl IntoIterator<Item = String>) -> ActionSchema {
    ActionSchema::Choice {
        options: items
            .into_iter()
            .map(|id| ChoiceOption {
                label: id.clone(),
                id,
                detail: None,
            })
            .collect(),
    }
}
fn list(item: ActionSchema, max: u32) -> ActionSchema {
    ActionSchema::List {
        min: 0,
        max,
        item: Box::new(item),
    }
}
fn err(case: &str, detail: &str) -> Rejection {
    Rejection::Engine(EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    })
}
fn hexes(c: &CnaContent, s: &State, seat: SeatId) -> Vec<HexId> {
    available(c, s, seat)
        .iter()
        .filter_map(|id| s.land.units[id].location.hex().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
/// Declare firing locations before inspecting anonymous enemy target classes. Empty role windows
/// remain present; private gun locations do not change another controller's decision schedule.
/// Cases: land:3.6, land:12.23, land:12.24
pub fn enter(
    c: &CnaContent,
    s: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    let generation = s
        .land
        .combat
        .barrage
        .generation
        .checked_add(1)
        .ok_or_else(|| EngineError::Invariant {
            detail: "barrage generation overflow".into(),
        })?;
    s.land.combat.barrage = BarrageState {
        generation,
        strict,
        ..Default::default()
    };
    for seat in seats() {
        let hs = hexes(c, s, seat);
        let n = hs.len() as u32;
        open(
            s,
            cx,
            seat,
            DECLARE,
            "Declare firing hexes; target classes are disclosed only after declarations close."
                .into(),
            &["land:12.23", "land:12.24"],
            Trigger::Scheduled,
            Secrecy::SecretSimultaneous,
            ActionSpace::new(list(ActionSchema::Hex { among: Some(hs) }, n))
                .with_pass("Declare no firing hexes."),
        );
    }
    Ok(())
}
/// Unbiased sampling without sharing controller randomness with adjudication.
/// Cases: land:12.24
fn index(rng: &mut CampaignRng, n: usize) -> usize {
    let mut range = 1u128;
    let mut digits = 0;
    while range < n as u128 {
        range *= 6;
        digits += 1
    }
    let limit = range - range % (n as u128);
    loop {
        let mut x = 0u128;
        for _ in 0..digits {
            x = x * 6 + u128::from(rng.d6().value() - 1)
        }
        if x < limit {
            return (x % (n as u128)) as usize;
        }
    }
}
fn classify(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
) -> Result<Vec<(BarrageTarget, Vec<String>)>, Rejection> {
    let u = &s.land.units[id];
    let Some(oa) = c.units.units.get(id) else {
        return Ok(vec![]);
    };
    if oa.stacking_points.is_none() || crate::view::toe_points(c, u).unwrap_or(0) == 0 {
        return Ok(vec![]);
    }
    let cl = super::super::formation::class(c, id)
        .ok_or_else(|| err("land:3.22", "target class unavailable"))?;
    if cl.unit_type == "headquarters" && cl.ca_def_paren && !matches!(u.toe, Some(Toe::Weapons(_)))
    {
        return Ok(vec![]);
    }
    let mut groups: BTreeMap<BarrageTarget, Vec<String>> = BTreeMap::new();
    if let Some(Toe::Weapons(ws)) = &u.toe {
        for p in ws.iter().filter(|p| p.n > 0) {
            let w = &c.units.weapons[&p.weapon];
            let class = if w.armor_prot.is_some_and(|n| n > 0) {
                BarrageTarget::Armor
            } else {
                BarrageTarget::Gun
            };
            groups.entry(class).or_default().push(p.weapon.clone());
        }
    } else {
        let class = if cl.armor_prot.is_some_and(|n| n > 0) || cl.unit_type == "tank" {
            BarrageTarget::Armor
        } else if cl.barrage.is_some_and(|n| n > 0)
            || matches!(
                cl.unit_type.as_str(),
                "artillery" | "anti_tank" | "anti_air"
            )
        {
            BarrageTarget::Gun
        } else {
            BarrageTarget::Infantry
        };
        groups.insert(class, vec![]);
    }
    Ok(groups.into_iter().collect())
}
/// Labels describe a broad class and ordinal, never a canonical unit identity. The private
/// association is shuffled afresh with checkpointed campaign dice, separately in each hex/class.
/// Cases: land:3.22, land:12.22, land:12.23, land:12.24
fn catalog(
    c: &CnaContent,
    s: &State,
    side: Side,
    rng: &mut CampaignRng,
) -> Result<Vec<Target>, Rejection> {
    let adjacent: BTreeSet<_> = s
        .land
        .combat
        .barrage
        .declarations
        .iter()
        .filter(|(seat, _)| seat.side == side)
        .flat_map(|(_, hs)| {
            hs.iter()
                .flat_map(|h| c.map.neighbors(h).into_iter().map(|r| r.id.clone()))
        })
        .collect();
    let mut grouped = BTreeMap::<(HexId, BarrageTarget), Vec<(UnitId, Vec<String>)>>::new();
    for u in s
        .units_of(side.opponent())
        .filter(|u| u.location.hex().is_some_and(|h| adjacent.contains(h)))
    {
        for (class, ws) in classify(c, s, &u.id)? {
            grouped
                .entry((u.location.hex().unwrap().clone(), class))
                .or_default()
                .push((u.id.clone(), ws));
        }
    }
    let mut targets = vec![];
    for ((hex, class), mut us) in grouped {
        for i in (1..us.len()).rev() {
            let j = index(rng, i + 1);
            us.swap(i, j)
        }
        let class_name = serde_json::to_value(class)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        for (i, (unit, weapons)) in us.into_iter().enumerate() {
            targets.push(Target {
                label: format!(
                    "b{}:{}:{}:{}",
                    s.land.combat.barrage.generation,
                    hex,
                    class_name,
                    i + 1
                ),
                hex: hex.clone(),
                class,
                unit,
                weapons,
            });
        }
    }
    Ok(targets)
}
pub fn disclosed(s: &State, p: Perspective) -> Value {
    let mut out = BTreeMap::new();
    for (side, ts) in &s.land.combat.barrage.targets {
        if crate::view::sees_side(p, *side) {
            out.insert(
                *side,
                ts.iter()
                    .map(|t| json!({"target":t.label,"hex":t.hex,"class":t.class}))
                    .collect::<Vec<_>>(),
            );
        }
    }
    json!(out)
}
/// Source identities and weapon choices are drawn only from the answering side's holdings.
/// Cases: land:12.13, land:12.15, airlog:50.13, airlog:50.15
fn space(c: &CnaContent, s: &State, seat: SeatId) -> ActionSpace {
    let ids = available(c, s, seat);
    let mut weapons = BTreeSet::new();
    let mut sources = BTreeSet::new();
    for id in &ids {
        if let Some(Toe::Weapons(ws)) = &s.land.units[id].toe {
            weapons.extend(ws.iter().map(|w| w.weapon.clone()));
        }
        if let Ok(ss) = logistics::available_sources_with_content(c, s, id) {
            sources.extend(ss.iter().map(|d| serde_json::to_string(&d.source).unwrap()));
        }
    }
    let draw = ActionSchema::Record {
        fields: vec![
            field("source", options(sources), false),
            field(
                "ammo",
                ActionSchema::Integer {
                    min: 1,
                    max: i32::MAX.into(),
                },
                false,
            ),
        ],
    };
    let gun = ActionSchema::Record {
        fields: vec![
            field("unit", ActionSchema::Unit { among: ids.clone() }, false),
            field("weapon", options(weapons), true),
            field(
                "toe",
                ActionSchema::Integer {
                    min: 1,
                    max: i32::MAX.into(),
                },
                false,
            ),
            field("draws", list(draw, 4096), false),
        ],
    };
    let targets = s
        .land
        .combat
        .barrage
        .targets
        .get(&seat.side)
        .into_iter()
        .flatten()
        .map(|t| t.label.clone());
    ActionSpace::new(list(
        ActionSchema::Record {
            fields: vec![
                field("target", options(targets), false),
                field("guns", list(gun, 4096), false),
            ],
        },
        4096,
    ))
    .with_pass("Do not barrage any target.")
}
/// Complete the declaration window, then present only source-authorized enemy counts/classes.
/// Cases: land:12.23, land:12.24
pub fn declare(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    a: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let hs: Vec<HexId> = if a.is_null() {
        vec![]
    } else {
        serde_json::from_value(a.clone()).map_err(|_| illegal("expected firing hex list"))?
    };
    let legal: BTreeSet<_> = hexes(c, s, p.seat).into_iter().collect();
    let chosen: BTreeSet<_> = hs.iter().cloned().collect();
    if chosen.len() != hs.len() || !chosen.is_subset(&legal) {
        return Err(illegal("select unique own firing hexes"));
    }
    s.land.combat.barrage.declarations.insert(p.seat, hs);
    if !s.decisions.pending.iter().any(|d| d.kind == DECLARE) {
        for side in Side::ALL {
            let firing: BTreeSet<_> = s
                .land
                .combat
                .barrage
                .declarations
                .iter()
                .filter(|(seat, _)| seat.side == side)
                .flat_map(|(_, hs)| hs.iter().cloned())
                .collect();
            cx.emit(EngineEvent::public(GameEvent::Note {
                text: format!("Barrage firing hexes {:?}: {}", side, json!(firing)),
            }));
            let ts = catalog(c, s, side, cx.rng)?;
            s.land.combat.barrage.targets.insert(side, ts);
            cx.emit(EngineEvent::new(
                Audience::Side(side),
                GameEvent::Note {
                    text: format!(
                        "Barrage target catalog: {}",
                        disclosed(s, Perspective::Side(side))
                    ),
                },
            ));
        }
        for seat in seats() {
            open(s,cx,seat,PLOT,"Plot secret barrages against disclosed anonymous targets; choose participating TOE and ammunition sources.".into(),&["land:12.13","land:12.14","land:12.15","land:12.16","land:12.23","land:12.32","airlog:50.13"],Trigger::Scheduled,Secrecy::SecretSimultaneous,space(c,s,seat));
        }
    }
    Ok("Firing hex declaration committed.".into())
}
fn component(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    weapon: &Option<String>,
) -> Result<(i32, i32), Rejection> {
    let u = s
        .land
        .units
        .get(id)
        .ok_or_else(|| illegal("unknown own gun"))?;
    if let Some(Toe::Weapons(ws)) = &u.toe {
        let name = weapon
            .as_ref()
            .ok_or_else(|| illegal("select a gun component"))?;
        let p = ws
            .iter()
            .find(|p| &p.weapon == name)
            .ok_or_else(|| illegal("unknown own weapon"))?;
        let rating = c.units.weapons[name].barrage.unwrap_or(0);
        Ok((rating, p.n))
    } else {
        if weapon.is_some() {
            return Err(illegal("this unit has no selectable weapon component"));
        }
        let rating = super::super::formation::class(c, id)
            .and_then(|cl| cl.barrage)
            .unwrap_or(0);
        let n = crate::view::toe_points(c, u)
            .ok_or_else(|| err("land:11.3", "gun TOE is unavailable"))?;
        Ok((rating, n))
    }
}
fn draw(c: &CnaContent, s: &mut State, g: &Contribution) -> Result<(), Rejection> {
    let ammo = logistics::ammunition_cost(
        c,
        AmmoMode::Played,
        AmmoAction::Barrage,
        ToeStrengthPoints::new(g.toe),
    )
    .map_err(|_| err("airlog:50.2", "barrage ammunition cost unavailable"))?;
    let draws = g
        .draws
        .iter()
        .map(|d| {
            Ok(logistics::SupplyDraw {
                source: serde_json::from_str(&d.source)
                    .map_err(|_| illegal("unknown own ammunition source"))?,
                amount: logistics::SupplyDemand {
                    ammo: AmmoPoints::new(d.ammo),
                    ..Default::default()
                },
            })
        })
        .collect::<Result<Vec<_>, Rejection>>()?;
    logistics::spend_for_unit_with_content(
        c,
        s,
        &g.unit,
        logistics::SupplyDemand {
            ammo,
            ..Default::default()
        },
        &draws,
    )
    .map_err(|_| illegal("invalid or insufficient own ammunition allocation"))
}
/// Recheck the whole side's private plans against a shadow state, reserving shared stocks in
/// canonical seat order. Neither accepted partial plans nor rejected answers spend ammunition.
/// Cases: land:11.34, land:12.13, land:12.14, land:12.15, land:12.16, land:12.31, land:12.32, airlog:50.13
fn validate(c: &CnaContent, s: &State) -> Result<State, Rejection> {
    let mut shadow = s.clone();
    let mut used = BTreeMap::<(UnitId, Option<String>), i32>::new();
    let mut targets = BTreeSet::new();
    let mut calls = BTreeMap::<UnitId, usize>::new();
    for (seat, plans) in &s.land.combat.barrage.plans {
        let own: BTreeSet<_> = available(c, s, *seat).into_iter().collect();
        for fire in plans {
            let target = s
                .land
                .combat
                .barrage
                .targets
                .get(&seat.side)
                .into_iter()
                .flatten()
                .find(|t| t.label == fire.target)
                .ok_or_else(|| illegal("unknown disclosed target"))?;
            shift(c, &target.hex)?;
            if fire.guns.is_empty() {
                return Err(illegal("a barrage needs assigned gun TOE"));
            }
            let mut origins = BTreeSet::new();
            let mut unique = BTreeSet::new();
            for g in &fire.guns {
                if g.draws.len() > 4096
                    || g.toe <= 0
                    || !own.contains(&g.unit)
                    || !unique.insert((g.unit.clone(), g.weapon.clone()))
                {
                    return Err(illegal("select positive unique own gun components"));
                }
                let (rating, n) = component(c, s, &g.unit, &g.weapon)?;
                let u = &s.land.units[&g.unit];
                let h = u.location.hex().unwrap();
                if rating <= 0
                    || !s.land.combat.barrage.declarations[seat].contains(h)
                    || !c.map.neighbors(h).iter().any(|x| x.id == target.hex)
                {
                    return Err(illegal(
                        "gun must be in a declared hex adjacent to the target",
                    ));
                }
                let assigned = used.entry((g.unit.clone(), g.weapon.clone())).or_default();
                *assigned = assigned
                    .checked_add(g.toe)
                    .ok_or_else(|| illegal("assigned TOE overflow"))?;
                if *assigned > n {
                    return Err(illegal("assigned gun TOE exceeds its strength"));
                }
                origins.insert(h.clone());
                draw(c, &mut shadow, g)?;
            }
            let units: BTreeSet<_> = fire.guns.iter().map(|g| g.unit.clone()).collect();
            for id in units {
                let count = calls.entry(id.clone()).or_default();
                *count += 1;
                let pos = s
                    .land
                    .combat
                    .positions
                    .get(&id)
                    .copied()
                    .unwrap_or_default();
                if pos == Position::Back && (*count > 1 || fire.guns.iter().any(|g| g.unit != id)) {
                    return Err(illegal("Back guns cannot combine or split fire"));
                }
                if pos == Position::Forward
                    && c.units.units[&id].nationality == "italian"
                    && origins.len() > 1
                {
                    return Err(illegal(
                        "Italian Forward guns coordinate only within one hex",
                    ));
                }
            }
            for origin in origins {
                if !targets.insert((seat.side, fire.target.clone(), origin)) {
                    return Err(illegal("target already fired on from this source hex"));
                }
            }
        }
    }
    let firing: BTreeSet<_> = s
        .land
        .combat
        .barrage
        .plans
        .iter()
        .flat_map(|(seat, fs)| {
            fs.iter().flat_map(|f| {
                f.guns.iter().map(|g| {
                    (
                        seat.side,
                        s.land.units[&g.unit].location.hex().unwrap().clone(),
                    )
                })
            })
        })
        .collect();
    for (side, hex) in firing {
        charge(c, &mut shadow, side, &hex, true)?;
    }
    Ok(shadow)
}
/// Plot answers remain private. All stock reservation and strength validation precede any dice.
/// Cases: land:3.6, land:12.0, land:12.45, airlog:50.13
pub fn answer(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    a: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let plans: Vec<Fire> = if a.is_null() {
        vec![]
    } else {
        serde_json::from_value(a.clone()).map_err(|_| illegal("expected barrage plot list"))?
    };
    if plans.len() > 4096 || plans.iter().any(|f| f.guns.len() > 4096) {
        return Err(illegal("too many barrage entries"));
    }
    s.land.combat.barrage.plans.insert(p.seat, plans);
    let paid = validate(c, s)?;
    if !s.decisions.pending.iter().any(|d| d.kind == PLOT) {
        resolve(c, s, paid, cx)?;
    }
    Ok("Private barrage plan committed.".into())
}

fn strength(c: &CnaContent, s: &State, id: &UnitId) -> Result<i32, Rejection> {
    crate::view::toe_points(c, &s.land.units[id]).ok_or_else(|| err("land:11.3", "TOE unavailable"))
}
/// Charge every unit in an affected hex, only the newly required segment CP. Barrage on its own
/// costs five CP when firing as phasing player, three when receiving or firing as nonphasing.
/// Cases: land:11.21, land:11.22, land:11.23, land:11.24, land:11.25, land:6.2
fn charge(
    c: &CnaContent,
    s: &mut State,
    side: Side,
    hex: &HexId,
    firing: bool,
) -> Result<(), Rejection> {
    let phasing = s.cursor.phasing(s.turn.player_a) == Some(side);
    let total = if phasing && firing { 20 } else { 12 };
    let ids: Vec<_> = s
        .units_of(side)
        .filter(|u| u.location.hex() == Some(hex))
        .map(|u| u.id.clone())
        .collect();
    for id in ids {
        let before = s.land.combat.cp_charged.get(&id).copied().unwrap_or(0);
        let delta = (total - before).max(0);
        if delta > 0 {
            let allowance = super::super::formation::individual_allowance(c, s, &id)
                .ok_or_else(|| err("land:11.2", "combat CPA unavailable"))?;
            // Attacking dry vehicles cannot opt into a barrage. Receiving fire remains mandatory;
            // the logistics restriction does not grant immunity to an enemy's attack.
            if firing {
                logistics::spend_activity_water(c, s, &id)
                    .map_err(|_| illegal("firing hex lacks required activity water"))?;
            }
            super::super::capability::charge(
                s.land.units.get_mut(&id).unwrap(),
                allowance,
                delta,
                false,
            )?;
            s.land.combat.cp_charged.insert(id, total);
        }
    }
    Ok(())
}
/// Target terrain uses its single best applicable column shift; trucks ignore it. Unknown base
/// terrain is not replaced by clear terrain. Fortification/facility procedures remain unsupported.
/// Cases: land:12.33, land:12.34, land:12.46
fn shift(c: &CnaContent, hex: &HexId) -> Result<i32, Rejection> {
    let terrain = super::super::map::terrain(c, hex, true)?;
    match c.tables.land.terrain_effects.feature(terrain).barrage_shift {
        CombatShift::Columns(n) => Ok(n),
        _ => Err(err(
            "land:12.33",
            "target terrain barrage instruction unavailable",
        )),
    }
}
fn roll(cx: &mut Cx<'_>, purpose: String) -> cna_core::dice::TwoDiceReading {
    let d = cx.rng.two_dice_reading();
    cx.emit(EngineEvent::public(GameEvent::DiceRolled {
        purpose,
        dice: vec![d.tens.value(), d.units.value()],
        reading: Some(d.value()),
        rule: Some("land:12.42".into()),
    }));
    d
}
/// All plans use the pre-casualty strength. Commit shared ammunition once, resolve in stable
/// seat order and each controller's requested target order, and defer loss choices until both sides have fired.
/// Cases: land:11.32, land:11.34, land:12.42, land:12.43, land:12.44, land:12.45, land:12.46
fn resolve(c: &CnaContent, s: &mut State, paid: State, cx: &mut Cx<'_>) -> Result<(), Rejection> {
    let plans = s.land.combat.barrage.plans.clone();
    let targets = s.land.combat.barrage.targets.clone();
    // CP/activity costs are reserved against the same shadow as ammunition before revealing dice.
    let mut paid = paid;
    let mut firing = BTreeSet::new();
    let mut receiving = BTreeSet::new();
    for (seat, fs) in &plans {
        for f in fs {
            let t = targets[&seat.side]
                .iter()
                .find(|t| t.label == f.target)
                .unwrap();
            receiving.insert((seat.side.opponent(), t.hex.clone()));
            for g in &f.guns {
                firing.insert((
                    seat.side,
                    s.land.units[&g.unit].location.hex().unwrap().clone(),
                ));
            }
        }
    }
    for (side, hex) in &firing {
        charge(c, &mut paid, *side, hex, true)?;
    }
    for (side, hex) in receiving {
        if !firing.contains(&(side, hex.clone())) {
            charge(c, &mut paid, side, &hex, false)?;
        }
    }
    s.logistics = paid.logistics;
    s.land.units = paid.land.units;
    s.land.combat.cp_charged = paid.land.combat.cp_charged;
    for id in s.land.combat.cp_charged.keys() {
        let u = &s.land.units[id];
        cx.emit(EngineEvent::new(
            Audience::Side(u.side),
            GameEvent::UnitUpdated {
                unit: crate::view::unit_view(c, u),
            },
        ));
    }
    for (seat, fs) in plans {
        for f in fs {
            let t = targets[&seat.side]
                .iter()
                .find(|t| t.label == f.target)
                .unwrap();
            let raw = c
                .tables
                .land
                .combat_calculations
                .raw_points(
                    f.guns
                        .iter()
                        .map(|g| (component(c, s, &g.unit, &g.weapon).unwrap().0, g.toe)),
                )
                .ok_or_else(|| illegal("barrage raw strength overflow"))?;
            let actual = c
                .tables
                .land
                .combat_calculations
                .actual_points(StrengthActivity::Barrage, raw)
                .ok_or_else(|| illegal("barrage strength overflow"))?;
            let d = roll(cx, format!("Barrage at {}", t.label));
            let result = c
                .tables
                .land
                .barrage
                .result(
                    actual,
                    shift(c, &t.hex)?,
                    t.class,
                    d,
                    s.land.units[&t.unit].transport_trucks.total() > 0,
                )
                .ok_or_else(|| err("land:12.6", "barrage table lookup failed"))?;
            let has_trucks = s
                .units_of(seat.side.opponent())
                .any(|u| u.location.hex() == Some(&t.hex) && u.trucks.total() > 0);
            if has_trucks && s.land.combat.barrage.strict {
                return Err(err(
                    "land:12.46",
                    "truck/cargo loss allocation is not implemented yet",
                ));
            }
            let trucks = if has_trucks {
                let td = roll(cx, format!("Concurrent truck barrage at {}", t.label));
                c.tables
                    .land
                    .barrage
                    .result(actual, 0, BarrageTarget::Truck, td, false)
                    .ok_or_else(|| err("land:12.46", "truck table lookup failed"))?
                    .toe_points_lost
            } else {
                0
            };
            if trucks > 0 || result.transport_truck_points_lost > 0 {
                cx.emit(EngineEvent::new(Audience::Side(seat.side.opponent()),GameEvent::Note{text:"Development gap land:12.46: truck and cargo losses have not been applied; allocation support is pending.".into()}));
            }
            s.land
                .combat
                .barrage
                .casualties
                .entry(t.unit.clone())
                .or_default()
                .push(Casualty {
                    weapons: t.weapons.clone(),
                    loss: result.toe_points_lost,
                    pinned: result.pinned,
                    trucks: result.transport_truck_points_lost + trucks,
                });
            if result.pinned {
                s.land.combat.pinned.insert(t.unit.clone());
            }
            // Enemy designation, gun composition and exact TOE stay private; only the anonymous
            // target, pooled points and table result are disclosed for this attack.
            cx.emit(EngineEvent::public(GameEvent::CombatResolved{hex:t.hex.to_string(),summary:format!("Barrage {}: {} actual points, {} TOE loss, pinned={}; concurrent truck loss {}.",t.label,actual,result.toe_points_lost,result.pinned,trucks),detail:None}));
            cx.emit(EngineEvent::new(
                Audience::Side(seat.side),
                GameEvent::Note {
                    text: format!("Own barrage plot: {}", json!(f)),
                },
            ));
            cx.emit(EngineEvent::new(
                Audience::Side(seat.side.opponent()),
                GameEvent::Note {
                    text: format!("Barrage result on own unit {}: {}", t.unit, json!(result)),
                },
            ));
        }
    }
    for seat in seats() {
        let (items, needed) = loss_space(c, s, seat)?;
        open(
            s,
            cx,
            seat,
            LOSSES,
            "Allocate own barrage losses after every simultaneous barrage has fired.".into(),
            &["land:12.44", "land:12.45", "land:12.46"],
            Trigger::Triggered,
            Secrecy::Secret,
            if needed {
                ActionSpace::new(items)
            } else {
                ActionSpace::new(items).with_pass("No own losses to allocate.")
            },
        );
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loss {
    pub unit: UnitId,
    pub weapon: Option<String>,
    pub toe: i32,
}
fn obligations(
    c: &CnaContent,
    s: &State,
    seat: SeatId,
) -> Result<Vec<(UnitId, Vec<String>, i32)>, Rejection> {
    let mut out = vec![];
    for (id, cs) in &s.land.combat.barrage.casualties {
        if s.land.units[id].side != seat.side || ownership::seat_for_unit(c, s, id) != seat.role {
            continue;
        }
        let mut groups = BTreeMap::<Vec<String>, i32>::new();
        for x in cs {
            let n = groups.entry(x.weapons.clone()).or_default();
            *n = n
                .checked_add(x.loss)
                .ok_or_else(|| illegal("casualty overflow"))?;
        }
        for (ws, n) in groups {
            let available = if ws.is_empty() {
                strength(c, s, id)?
            } else {
                match &s.land.units[id].toe {
                    Some(Toe::Weapons(ps)) => ps
                        .iter()
                        .filter(|p| ws.contains(&p.weapon))
                        .map(|p| p.n)
                        .sum(),
                    _ => return Err(illegal("target composition changed")),
                }
            };
            out.push((id.clone(), ws, n.min(available)));
        }
    }
    Ok(out)
}
fn loss_space(c: &CnaContent, s: &State, seat: SeatId) -> Result<(ActionSchema, bool), Rejection> {
    let os = obligations(c, s, seat)?;
    let ids = os
        .iter()
        .map(|(id, _, _)| id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let weapons = os
        .iter()
        .flat_map(|(_, ws, _)| ws.clone())
        .collect::<BTreeSet<_>>();
    let needed = os.iter().any(|(_, _, n)| *n > 0);
    Ok((
        list(
            ActionSchema::Record {
                fields: vec![
                    field("unit", ActionSchema::Unit { among: ids }, false),
                    field("weapon", options(weapons), true),
                    field(
                        "toe",
                        ActionSchema::Integer {
                            min: 1,
                            max: i32::MAX.into(),
                        },
                        false,
                    ),
                ],
            },
            4096,
        ),
        needed,
    ))
}
/// The defender chooses which eligible components absorb each actual TOE casualty. Rejecting an
/// allocation never rerolls combat or applies only part of an allocation.
/// Cases: land:3.6, land:12.45
pub fn losses(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    a: &Value,
    _cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let ls: Vec<Loss> = if a.is_null() {
        vec![]
    } else {
        serde_json::from_value(a.clone())
            .map_err(|_| illegal("expected own casualty allocation"))?
    };
    let os = obligations(c, s, p.seat)?;
    let mut selected = BTreeMap::<(UnitId, Option<String>), i32>::new();
    for l in &ls {
        if l.toe <= 0
            || !os.iter().any(|(id, ws, _)| {
                id == &l.unit
                    && (if ws.is_empty() {
                        l.weapon.is_none()
                    } else {
                        l.weapon.as_ref().is_some_and(|w| ws.contains(w))
                    })
            })
        {
            return Err(illegal("invalid own casualty component"));
        }
        let n = selected
            .entry((l.unit.clone(), l.weapon.clone()))
            .or_default();
        *n = n
            .checked_add(l.toe)
            .ok_or_else(|| illegal("loss overflow"))?;
    }
    for (id, ws, n) in &os {
        let assigned = selected
            .iter()
            .filter(|((u, w), _)| {
                u == id
                    && (if ws.is_empty() {
                        w.is_none()
                    } else {
                        w.as_ref().is_some_and(|w| ws.contains(w))
                    })
            })
            .try_fold(0i32, |sum, (_, n)| sum.checked_add(*n))
            .ok_or_else(|| illegal("loss overflow"))?;
        if assigned != *n {
            return Err(illegal("allocate exactly each own required casualty"));
        }
    }
    for ((id, w), n) in selected {
        let current = strength(c, s, &id)?;
        let u = s.land.units.get_mut(&id).unwrap();
        if let Some(w) = w {
            let Some(Toe::Weapons(ps)) = &mut u.toe else {
                return Err(illegal("unit is not weapon-composed"));
            };
            let p = ps.iter_mut().find(|p| p.weapon == w).unwrap();
            if n > p.n {
                return Err(illegal("component casualty exceeds strength"));
            }
            p.n -= n;
        } else {
            if n > current {
                return Err(illegal("casualty exceeds strength"));
            }
            u.toe = Some(Toe::Under { under: current - n });
        }
    }
    for (id, _, _) in os {
        let u = &s.land.units[&id];
        let mut unit = crate::view::unit_view(c, u);
        super::stamp_view(s, &id, &mut unit);
        _cx.emit(EngineEvent::new(
            Audience::Side(u.side),
            GameEvent::UnitUpdated { unit },
        ));
    }
    Ok("Own barrage TOE casualties allocated.".into())
}
/// Choose one own component and one adjacent anonymous target. Prefer its own ready ammunition
/// so independently acting same-side seats cannot overbook shared stocks.
/// Cases: land:12.13, land:12.15, land:12.23, airlog:50.13
pub fn random_plans(
    c: &CnaContent,
    s: &State,
    r: &cna_core::decision::DecisionRequest,
    rng: &mut CampaignRng,
) -> Value {
    if r.kind == DECLARE {
        return json!(hexes(c, s, r.seat));
    }
    if r.kind == LOSSES {
        let Ok(os) = obligations(c, s, r.seat) else {
            return Value::Null;
        };
        let mut ls = vec![];
        for (unit, ws, mut need) in os {
            if ws.is_empty() {
                if need > 0 {
                    ls.push(Loss {
                        unit,
                        weapon: None,
                        toe: need,
                    })
                }
            } else {
                for w in ws {
                    let Some(Toe::Weapons(ps)) = &s.land.units[&unit].toe else {
                        continue;
                    };
                    let n = ps
                        .iter()
                        .find(|p| p.weapon == w)
                        .map_or(0, |p| p.n)
                        .min(need);
                    if n > 0 {
                        ls.push(Loss {
                            unit: unit.clone(),
                            weapon: Some(w),
                            toe: n,
                        });
                        need -= n
                    }
                }
            }
        }
        return json!(ls);
    }
    if r.kind != PLOT {
        return Value::Null;
    }
    let mut ids = available(c, s, r.seat);
    while !ids.is_empty() {
        let id = ids.remove(index(rng, ids.len()));
        let u = &s.land.units[&id];
        let Some(h) = u.location.hex() else { continue };
        if !s
            .land
            .combat
            .barrage
            .declarations
            .get(&r.seat)
            .is_some_and(|hs| hs.contains(h))
        {
            continue;
        }
        let targets: Vec<_> = s
            .land
            .combat
            .barrage
            .targets
            .get(&r.seat.side)
            .into_iter()
            .flatten()
            .filter(|t| {
                c.map.neighbors(h).iter().any(|x| x.id == t.hex) && shift(c, &t.hex).is_ok()
            })
            .collect();
        if targets.is_empty() {
            continue;
        }
        let weapons: Vec<Option<String>> = if let Some(Toe::Weapons(ps)) = &u.toe {
            ps.iter().map(|p| Some(p.weapon.clone())).collect()
        } else {
            vec![None]
        };
        for weapon in weapons {
            let Ok((rating, toe)) = component(c, s, &id, &weapon) else {
                continue;
            };
            if rating <= 0 || toe <= 0 {
                continue;
            }
            let Ok(ammo) = logistics::ammunition_cost(
                c,
                AmmoMode::Played,
                AmmoAction::Barrage,
                ToeStrengthPoints::new(toe),
            ) else {
                continue;
            };
            if s.logistics
                .unit_supply
                .get(&id)
                .map_or(0, |s| s.ready_ammo.get())
                < ammo.get()
            {
                continue;
            }
            let f = Fire {
                target: targets[index(rng, targets.len())].label.clone(),
                guns: vec![Contribution {
                    unit: id.clone(),
                    weapon,
                    toe,
                    draws: vec![Draw {
                        source: serde_json::to_string(&logistics::SupplySource::ReadyAmmo).unwrap(),
                        ammo: ammo.get(),
                    }],
                }],
            };
            let mut shadow = s.clone();
            shadow
                .land
                .combat
                .barrage
                .plans
                .insert(r.seat, vec![f.clone()]);
            if validate(c, &shadow).is_ok() {
                return json!([f]);
            }
        }
    }
    json!([])
}

#[cfg(test)]
#[path = "barrage_tests.rs"]
mod barrage_tests;
