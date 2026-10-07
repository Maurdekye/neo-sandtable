//! Secret disjoint TOE assignments; answers never resolve fire or inspect enemy contents.
use super::{Position, barrage::Draw};
use crate::land::{
    formation,
    reserve::{self, ReserveCombat},
};
use crate::{
    CnaContent, State, logistics,
    state::Pending,
    steps::{illegal, open},
};
use cna_content::units::Toe;
use cna_core::{
    decision::{
        ActionSchema, ActionSpace, ChoiceOption, DecisionRequest, FieldSchema, Secrecy, Trigger,
    },
    dice::CampaignRng,
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{HexId, SeatId, UnitId},
    quantity::{AmmoPoints, ToeStrengthPoints},
    visibility::{Audience, Perspective},
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::airlog::supply::{AmmoAction, AmmoMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "cna.combat.force_assignment";
pub const ANCHOR: &str = "opstage.movement_and_combat.combat.force_assignment";
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombatRole {
    AntiArmor,
    CloseAssault,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub unit: UnitId,
    #[serde(default)]
    pub weapon: Option<String>,
    pub role: CombatRole,
    pub target: HexId,
    pub toe: i32,
    pub draws: Vec<Draw>,
    /// The phasing side's private assault ordering/group index. Same index means one attack.
    #[serde(default)]
    pub assault: Option<u32>,
    #[serde(default)]
    pub probe: Option<bool>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AssignmentState {
    pub entered: bool,
    pub closed: bool,
    pub frozen: bool,
    pub plans: BTreeMap<Side, Vec<Assignment>>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Partition {
    pub unit: UnitId,
    pub weapon: Option<String>,
    pub anti_armor: i32,
    pub close_assault: i32,
    pub withheld: i32,
}
#[derive(Debug, Clone, Serialize)]
pub struct Assault {
    pub index: u32,
    pub targets: BTreeSet<HexId>,
    pub probe: bool,
}
struct Component {
    toe: i32,
    aa: i32,
    aa_paren: bool,
    ca: i32,
    ca_paren: bool,
}
fn components(c: &CnaContent, s: &State, id: &UnitId) -> Vec<Option<String>> {
    match &s.land.units[id].toe {
        Some(Toe::Weapons(ws)) => ws
            .iter()
            .filter(|w| w.n > 0)
            .map(|w| Some(w.weapon.clone()))
            .collect(),
        _ => {
            if formation::strength(c, s, id) > 0 {
                vec![None]
            } else {
                vec![]
            }
        }
    }
}
fn component(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    weapon: &Option<String>,
    phasing: bool,
) -> Result<Component, Rejection> {
    let unit = s
        .land
        .units
        .get(id)
        .ok_or_else(|| illegal("unknown own component"))?;
    if let Some(Toe::Weapons(ws)) = &unit.toe {
        let name = weapon
            .as_ref()
            .ok_or_else(|| illegal("select an actual weapon component"))?;
        let point = ws
            .iter()
            .find(|w| &w.weapon == name)
            .ok_or_else(|| illegal("unknown own weapon component"))?;
        let w = c
            .units
            .weapons
            .get(name)
            .ok_or_else(|| illegal("own weapon ratings unavailable"))?;
        return Ok(Component {
            toe: point.n,
            aa: w.anti_armor.unwrap_or(0),
            aa_paren: w.anti_armor_paren,
            ca: if phasing { w.ca_off } else { w.ca_def }.unwrap_or(0),
            ca_paren: if phasing {
                w.ca_off_paren
            } else {
                w.ca_def_paren
            },
        });
    }
    if weapon.is_some() {
        return Err(illegal("this unit has no selectable weapon component"));
    }
    let cl = formation::class(c, id).ok_or_else(|| illegal("own unit ratings unavailable"))?;
    let toe = logistics::toe_strength(c, unit)
        .map_err(|_| illegal("own TOE unavailable"))?
        .get();
    Ok(Component {
        toe,
        aa: cl.anti_armor.unwrap_or(0),
        aa_paren: cl.anti_armor_paren,
        ca: if phasing { cl.ca_off } else { cl.ca_def }.unwrap_or(0),
        ca_paren: if phasing {
            cl.ca_off_paren
        } else {
            cl.ca_def_paren
        },
    })
}
fn phasing(s: &State, side: Side) -> bool {
    s.cursor.phasing(s.turn.player_a) == Some(side)
}
fn eligible(c: &CnaContent, s: &State, side: Side) -> Vec<UnitId> {
    s.units_of(side)
        .filter(|u| {
            u.location.hex().is_some()
                && formation::strength(c, s, &u.id) > 0
                && !s.land.combat.pinned.contains(&u.id)
                && !s.land.combat.retreat.retreated.contains(&u.id)
                && s.land.combat.positions.get(&u.id) != Some(&Position::Back)
        })
        .filter(|u| {
            components(c, s, &u.id).iter().any(|w| {
                component(c, s, &u.id, w, phasing(s, side)).is_ok_and(|p| p.aa > 0 || p.ca > 0)
            })
        })
        .map(|u| u.id.clone())
        .collect()
}
/// Target domains use public presence only. Defensive assignments reference their own hex.
/// Terrain and fire resolution remain separate procedures; no armor-presence query occurs here.
/// Cases: land:14.12, land:14.23, land:14.27, land:15.21
fn targets(c: &CnaContent, s: &State, id: &UnitId, side: Side) -> Vec<HexId> {
    let Some(origin) = s.land.units[id].location.hex() else {
        return vec![];
    };
    if !phasing(s, side) {
        return vec![origin.clone()];
    }
    let enemy: BTreeSet<_> = s
        .stacks()
        .keys()
        .filter(|(_, who)| *who == side.opponent())
        .map(|(h, _)| h.clone())
        .collect();
    c.map
        .neighbors(origin)
        .iter()
        .filter(|h| enemy.contains(&h.id))
        .map(|h| h.id.clone())
        .collect()
}
fn field(name: &str, doc: &str, schema: ActionSchema, optional: bool) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: doc.into(),
        schema,
        optional,
    }
}
fn choices(values: BTreeSet<String>) -> ActionSchema {
    ActionSchema::Choice {
        options: values
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
        item: Box::new(item),
        min: 0,
        max,
    }
}
fn space(c: &CnaContent, s: &State, side: Side) -> ActionSpace {
    let ids = eligible(c, s, side);
    let mut weapons = BTreeSet::new();
    let mut hexes = BTreeSet::new();
    let mut sources = BTreeSet::new();
    for id in &ids {
        weapons.extend(components(c, s, id).into_iter().flatten());
        hexes.extend(targets(c, s, id, side));
        if let Ok(available) = logistics::available_sources_with_content(c, s, id) {
            sources.extend(
                available
                    .into_iter()
                    .map(|x| serde_json::to_string(&x.source).unwrap()),
            );
        }
    }
    let max = if ids.is_empty() || hexes.is_empty() {
        0
    } else {
        4096
    };
    let draw = ActionSchema::Record {
        fields: vec![
            field(
                "source",
                "One own ammunition source, encoded as its listed JSON identity.",
                choices(sources),
                false,
            ),
            field(
                "ammo",
                "Ammunition points drawn for this participating TOE.",
                ActionSchema::Integer {
                    min: 1,
                    max: i32::MAX.into(),
                },
                false,
            ),
        ],
    };
    let item = ActionSchema::Record {
        fields: vec![
            field(
                "unit",
                "One actual own unit; assign each component's TOE without duplication.",
                ActionSchema::Unit { among: ids },
                false,
            ),
            field(
                "weapon",
                "Actual weapon identity for mixed weapon TOE; omit for body TOE.",
                choices(weapons),
                true,
            ),
            field(
                "role",
                "One disjoint combat role for these TOE points.",
                choices(BTreeSet::from([
                    "anti_armor".into(),
                    "close_assault".into(),
                ])),
                false,
            ),
            field(
                "target",
                "Phasing: adjacent public enemy hex. Defensive: own unit's hex.",
                ActionSchema::Hex {
                    among: Some(hexes.into_iter().collect()),
                },
                false,
            ),
            field(
                "toe",
                "Positive participating TOE; unassigned residue is privately withheld.",
                ActionSchema::Integer {
                    min: 1,
                    max: i32::MAX.into(),
                },
                false,
            ),
            field(
                "draws",
                "Exact ammunition reservation from own sources, spent only when the action fires.",
                list(draw, 4096),
                false,
            ),
            field(
                "assault",
                "Phasing close assault only: contiguous group indices from zero, in desired order.",
                ActionSchema::Integer { min: 0, max: 4095 },
                true,
            ),
            field(
                "probe",
                "Phasing close assault only: explicit private Probe designation for this group.",
                ActionSchema::Bool,
                true,
            ),
        ],
    };
    ActionSpace::new(list(item, max))
        .with_pass("Withhold all uncommitted TOE; defensive casualty exposure remains.")
        .with_context(json!({"forced_pass":max==0,"phasing":phasing(s,side)}))
}
/// Two fixed FrontLine windows match the force-assignment registry, regardless of enemy contents.
/// Cases: land:14.11, land:14.26, land:15.16, land:3.6
/// Unsupported: land:14.3, land:15.3 - fire terrain and subsequent combat adjudication are next slices.
pub fn enter(
    c: &CnaContent,
    s: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    if strict {
        return Err(EngineError::Unsupported { case: "land:14.3".into(),
        detail: "Force plans are implemented; attack terrain and anti-armor/assault adjudication remain incomplete.".into() });
    }
    if s.cursor.phasing(s.turn.player_a).is_none() {
        return Ok(());
    }
    s.land.combat.assignment = AssignmentState {
        entered: true,
        ..Default::default()
    };
    for side in Side::ALL {
        open(
            s,
            cx,
            SeatId::new(side, Role::FrontLine),
            KIND,
            "Privately partition actual weapon/body TOE into anti-armor and close-assault groups; residue is withheld. Fire resolution follows separately.".into(),
            &[
                "land:14.11",
                "land:14.26",
                "land:14.27",
                "land:15.16",
                "land:15.24",
                "land:15.25",
            ],
            Trigger::Scheduled,
            Secrecy::SecretSimultaneous,
            space(c, s, side),
        );
    }
    Ok(())
}
fn ammo_action(
    c: &CnaContent,
    id: &UnitId,
    _weapon: &Option<String>,
    role: CombatRole,
) -> Result<AmmoAction, Rejection> {
    if role == CombatRole::AntiArmor {
        return Ok(AmmoAction::AntiArmor);
    }
    logistics::close_assault_ammo_action(c, id)
        .map_err(|_| illegal("own ammunition classification remains unsupported (airlog:50.2)"))
}
/// Reserve exact participating TOE ammunition on a clone; never consume actual stocks in Respond.
/// Cases: airlog:50.13, airlog:50.14, airlog:50.15, airlog:50.2, land:14.24, land:15.15
fn reserve_ammo(c: &CnaContent, shadow: &mut State, a: &Assignment) -> Result<(), Rejection> {
    let ammo = logistics::ammunition_cost(
        c,
        AmmoMode::Played,
        ammo_action(c, &a.unit, &a.weapon, a.role)?,
        ToeStrengthPoints::new(a.toe),
    )
    .map_err(|_| illegal("own ammunition cost unavailable"))?;
    let draws = a
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
        shadow,
        &a.unit,
        logistics::SupplyDemand {
            ammo,
            ..Default::default()
        },
        &draws,
    )
    .map_err(|_| illegal("invalid or insufficient own ammunition reservation"))
}
/// Pure own-side validation: disjoint component pools, public geometry, own capabilities/stocks.
/// Hidden armor, rating composition, enemy assignments and outcomes are never inspected.
/// Cases: land:12.16, land:12.44, land:13.28, land:14.13, land:14.15, land:14.26, land:14.27,
/// Cases: land:15.12, land:15.16, land:15.17, land:15.23, land:15.24, airlog:52.51
fn validate(c: &CnaContent, s: &State, side: Side, plan: &[Assignment]) -> Result<(), Rejection> {
    if plan.len() > 4096 {
        return Err(illegal("too many force allocations"));
    }
    let own: BTreeSet<_> = eligible(c, s, side).into_iter().collect();
    let phase = phasing(s, side);
    let mut used = BTreeMap::<(UnitId, Option<String>), i32>::new();
    let mut groups = BTreeMap::<u32, (BTreeSet<HexId>, BTreeSet<UnitId>, bool)>::new();
    let mut shadow = s.clone();
    let mut aa_units = BTreeSet::new();
    for a in plan {
        if a.toe <= 0 || a.draws.len() > 4096 || !own.contains(&a.unit) {
            return Err(illegal("select positive eligible own TOE"));
        }
        let p = component(c, s, &a.unit, &a.weapon, phase)?;
        let total = used.entry((a.unit.clone(), a.weapon.clone())).or_default();
        *total = total
            .checked_add(a.toe)
            .ok_or_else(|| illegal("TOE allocation overflow"))?;
        if *total > p.toe {
            return Err(illegal(
                "a TOE point cannot serve more than one role or target",
            ));
        }
        if !targets(c, s, &a.unit, side).contains(&a.target) {
            return Err(illegal(
                "select a public adjacent target or your own defensive hex",
            ));
        }
        let origin = s.land.units[&a.unit].location.hex().unwrap();
        if a.role == CombatRole::AntiArmor {
            if p.aa <= 0 || a.assault.is_some() || a.probe.is_some() {
                return Err(illegal(
                    "anti-armor allocation needs its own rating and no assault metadata",
                ));
            }
            if p.aa_paren
                && s.units_of(side)
                    .filter(|u| u.location.hex() == Some(origin))
                    .any(|u| {
                        components(c, s, &u.id).iter().any(|w| {
                            component(c, s, &u.id, w, phase).is_ok_and(|p| p.aa > 0 && !p.aa_paren)
                        })
                    })
            {
                return Err(illegal(
                    "parenthesized anti-armor rating cannot accompany normal anti-armor TOE",
                ));
            }
            aa_units.insert(a.unit.clone());
        } else {
            if p.ca <= 0 {
                return Err(illegal("selected component has no close-assault rating"));
            }
            if p.ca_paren
                && s.units_of(side).any(|u| {
                    u.id != a.unit
                        && u.location.hex() == Some(origin)
                        && formation::combat_unit(c, &u.id)
                        && formation::strength(c, s, &u.id) > 0
                })
            {
                return Err(illegal(
                    "parenthesized close-assault rating requires no own combat unit in its hex",
                ));
            }
            if phase {
                if !logistics::movement_restrictions(c, s, &a.unit)
                    .map_err(|_| illegal("own offensive supply restriction unavailable"))?
                    .may_offensive_close_assault
                {
                    return Err(illegal(
                        "own ration or water state prohibits offensive close assault",
                    ));
                }
                let group = a
                    .assault
                    .filter(|i| *i < 4096)
                    .ok_or_else(|| illegal("phasing assault needs a private order index"))?;
                let probe = a.probe.unwrap_or(false);
                let g = groups
                    .entry(group)
                    .or_insert_with(|| (BTreeSet::new(), BTreeSet::new(), probe));
                if g.2 != probe {
                    return Err(illegal("one assault group has one probe designation"));
                }
                g.0.insert(a.target.clone());
                g.1.insert(a.unit.clone());
            } else if a.assault.is_some() || a.probe.is_some() {
                return Err(illegal(
                    "only the phasing side orders assaults and designates probes",
                ));
            }
        }
        reserve_ammo(c, &mut shadow, a)?;
    }
    let mut targeted = BTreeSet::new();
    for (expected, (group, (hexes, units, probe))) in groups.into_iter().enumerate() {
        if group as usize != expected {
            return Err(illegal(
                "assault order indices must be contiguous from zero",
            ));
        }
        for h in &hexes {
            if !targeted.insert(h.clone()) {
                return Err(illegal("one hex cannot be assigned to two assault groups"));
            }
            if hexes
                .iter()
                .any(|other| other != h && !c.map.neighbors(h).iter().any(|x| &x.id == other))
            {
                return Err(illegal(
                    "targets in a combined assault must be mutually adjacent",
                ));
            }
            for id in &units {
                if !c
                    .map
                    .neighbors(s.land.units[id].location.hex().unwrap())
                    .iter()
                    .any(|x| &x.id == h)
                {
                    return Err(illegal(
                        "each attacking unit must be adjacent to every target in its group",
                    ));
                }
            }
        }
        for id in units {
            reserve::record_offensive_action(
                &mut shadow,
                &id,
                if probe {
                    ReserveCombat::Probe
                } else {
                    ReserveCombat::CloseAssault
                },
            )?;
        }
    }
    if phase {
        for id in aa_units {
            reserve::record_offensive_action(&mut shadow, &id, ReserveCombat::AntiArmor)?;
        }
    }
    Ok(())
}
/// Commit only the answering side's private intended partition. Closure is not adjudication.
/// Cases: land:14.11, land:14.26, land:15.16, land:3.6
pub fn answer(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    action: &Value,
) -> Result<String, Rejection> {
    if p.seat.role != Role::FrontLine {
        return Err(illegal("force assignment belongs to FrontLine"));
    }
    let plan = if action.is_null() {
        vec![]
    } else {
        serde_json::from_value(action.clone())
            .map_err(|_| illegal("expected a private force assignment list"))?
    };
    validate(c, s, p.seat.side, &plan)?;
    s.land.combat.assignment.plans.insert(p.seat.side, plan);
    s.land.combat.assignment.closed = !s.decisions.pending.iter().any(|r| r.kind == KIND);
    Ok("Force partition recorded privately.".into())
}
/// Freeze the already validated plans without dice, spending, reveals or enemy-dependent stops.
/// Cases: land:14.26, land:15.16, land:3.6
pub fn finish(s: &mut State, cx: &mut Cx<'_>, strict: bool) -> Result<(), EngineError> {
    if strict {
        return Err(EngineError::Unsupported {
            case: "land:14.3".into(),
            detail: "Attack terrain and fire adjudication remain incomplete.".into(),
        });
    }
    if s.cursor.phasing(s.turn.player_a).is_none() || s.land.combat.assignment.frozen {
        return Ok(());
    }
    if !s.land.combat.assignment.closed {
        return Err(EngineError::Invariant {
            detail: "force assignment plans are not closed".into(),
        });
    }
    // Only the owner and operator receive identified unit annotations. This reveals no
    // enemy composition, and canonical ordering does not depend on submission order.
    for (side, plan) in &s.land.combat.assignment.plans {
        let units: BTreeSet<_> = plan.iter().map(|a| &a.unit).collect();
        for id in units {
            let event = EngineEvent::new(
                Audience::Side(*side),
                GameEvent::Note {
                    text: "Own force partition frozen for later combat adjudication.".into(),
                },
            )
            .about(id.clone());
            if let Some(hex) = s.land.units[id].location.hex() {
                cx.emit(event.at(hex.clone()));
            }
        }
    }
    s.land.combat.assignment.frozen = true;
    Ok(())
}
/// Owning side's unassigned actual TOE, including points withheld for lack of ammunition.
/// Casualty eligibility is decided by the later combat procedure, not by this residue.
/// Cases: land:14.21, land:14.26, land:15.12, land:15.15, land:15.16
pub fn partitions(c: &CnaContent, s: &State, side: Side) -> Vec<Partition> {
    let plan = s
        .land
        .combat
        .assignment
        .plans
        .get(&side)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    s.units_of(side)
        .filter(|u| u.location.hex().is_some())
        .flat_map(|u| {
            components(c, s, &u.id).into_iter().map(|weapon| {
                let aa = plan
                    .iter()
                    .filter(|a| {
                        a.unit == u.id && a.weapon == weapon && a.role == CombatRole::AntiArmor
                    })
                    .map(|a| a.toe)
                    .sum();
                let ca = plan
                    .iter()
                    .filter(|a| {
                        a.unit == u.id && a.weapon == weapon && a.role == CombatRole::CloseAssault
                    })
                    .map(|a| a.toe)
                    .sum();
                let toe = component(c, s, &u.id, &weapon, phasing(s, side)).map_or(0, |p| p.toe);
                Partition {
                    unit: u.id.clone(),
                    weapon,
                    anti_armor: aa,
                    close_assault: ca,
                    withheld: toe - aa - ca,
                }
            })
        })
        .collect()
}
/// The half-strength test counts participating units once, and commitments across all assaults.
/// Withheld and differently assigned TOE remains in each participating unit's denominator.
/// Cases: land:15.25, land:15.91
/// Interpretations: interp:land-0010
pub fn assaults(c: &CnaContent, s: &State, side: Side) -> Vec<Assault> {
    let plan = s
        .land
        .combat
        .assignment
        .plans
        .get(&side)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let close: Vec<_> = plan
        .iter()
        .filter(|a| a.role == CombatRole::CloseAssault && a.assault.is_some())
        .collect();
    let units: BTreeSet<_> = close.iter().map(|a| a.unit.clone()).collect();
    let available: i64 = units
        .iter()
        .map(|id| i64::from(formation::strength(c, s, id)))
        .sum();
    let committed: i64 = close.iter().map(|a| i64::from(a.toe)).sum();
    let automatic = committed * 2 < available;
    let mut groups = BTreeMap::<u32, Assault>::new();
    for a in close {
        let index = a.assault.unwrap();
        let g = groups.entry(index).or_insert_with(|| Assault {
            index,
            targets: BTreeSet::new(),
            probe: automatic || a.probe.unwrap_or(false),
        });
        g.targets.insert(a.target.clone());
    }
    groups.into_values().collect()
}
pub fn disclosed(c: &CnaContent, s: &State, p: Perspective) -> Value {
    if !s.land.combat.assignment.entered {
        return json!({"plans":{},"partitions":{},"assaults":{}});
    }
    let own = |side| match p {
        Perspective::Operator => true,
        Perspective::Side(who) => who == side,
        Perspective::Seat(seat) => seat.side == side,
    };
    json!({"plans":s.land.combat.assignment.plans.iter().filter(|(side,_)|own(**side)).collect::<BTreeMap<_,_>>(),
        "partitions":Side::ALL.into_iter().filter(|side|own(*side)).map(|side|(side,partitions(c,s,side))).collect::<BTreeMap<_,_>>(),
        "assaults":Side::ALL.into_iter().filter(|side|own(*side)).map(|side|(side,assaults(c,s,side))).collect::<BTreeMap<_,_>>()})
}
fn index(rng: &mut CampaignRng, n: usize) -> usize {
    let mut space = 6usize;
    let mut digits = 1;
    while space < n {
        space *= 6;
        digits += 1;
    }
    loop {
        let mut value = 0;
        for _ in 0..digits {
            value = value * 6 + usize::from(rng.d6().value() - 1);
        }
        if value < space - space % n {
            return value % n;
        }
    }
}
/// Choose one real own component and public target, with no enemy rating or armor query.
/// Cases: land:14.11,land:14.26,land:15.16,airlog:50.14
pub fn random_orders(
    c: &CnaContent,
    s: &State,
    r: &DecisionRequest,
    rng: &mut CampaignRng,
) -> Value {
    if r.kind != KIND {
        return Value::Null;
    }
    let mut ids = eligible(c, s, r.seat.side);
    while !ids.is_empty() {
        let id = ids.remove(index(rng, ids.len()));
        let mut ws = components(c, s, &id);
        while !ws.is_empty() {
            let weapon = ws.remove(index(rng, ws.len()));
            let mut hs = targets(c, s, &id, r.seat.side);
            while !hs.is_empty() {
                let target = hs.remove(index(rng, hs.len()));
                let roles = if rng.d6().value() <= 3 {
                    [CombatRole::AntiArmor, CombatRole::CloseAssault]
                } else {
                    [CombatRole::CloseAssault, CombatRole::AntiArmor]
                };
                for role in roles {
                    let Ok(p) = component(c, s, &id, &weapon, phasing(s, r.seat.side)) else {
                        continue;
                    };
                    let Ok(action) = ammo_action(c, &id, &weapon, role) else {
                        continue;
                    };
                    let Ok(ammo) = logistics::ammunition_cost(
                        c,
                        AmmoMode::Played,
                        action,
                        ToeStrengthPoints::new(p.toe),
                    ) else {
                        continue;
                    };
                    if s.logistics
                        .unit_supply
                        .get(&id)
                        .map_or(0, |x| x.ready_ammo.get())
                        < ammo.get()
                    {
                        continue;
                    }
                    let ca = role == CombatRole::CloseAssault && phasing(s, r.seat.side);
                    let a = Assignment {
                        unit: id.clone(),
                        weapon: weapon.clone(),
                        role,
                        target: target.clone(),
                        toe: p.toe,
                        draws: vec![Draw {
                            source: serde_json::to_string(&logistics::SupplySource::ReadyAmmo)
                                .unwrap(),
                            ammo: ammo.get(),
                        }],
                        assault: ca.then_some(0),
                        probe: ca.then(|| rng.d6().value() <= 2),
                    };
                    if validate(c, s, r.seat.side, std::slice::from_ref(&a)).is_ok() {
                        return json!([a]);
                    }
                }
            }
        }
    }
    json!([])
}
#[cfg(test)]
#[path = "assignment_tests.rs"]
mod tests;
