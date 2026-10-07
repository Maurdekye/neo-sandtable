use super::*;
use crate::{
    logistics::{
        CargoPacking,
        distribution::{self, Endpoint},
    },
    seq::{Block, OPSTAGE},
};
use cna_content::{
    scenario::Supplies,
    units::{Trucks, WeaponPoints},
};
use cna_core::{dice::CampaignRng, engine::Cx, quantity::AmmoPoints};
use serde_json::json;
fn setup() -> (CnaContent, State) {
    let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = State::new(&c).unwrap();
    s.cursor.block = Block::OpStage;
    s.cursor.op_stage = Some(1);
    s.cursor.index = OPSTAGE
        .iter()
        .position(|p| p.anchor == "opstage.organization.supply_distribution")
        .unwrap();
    (c, s)
}
fn ordinary(c: &CnaContent, s: &State) -> UnitId {
    s.land
        .units
        .keys()
        .find(|id| c.units.units[*id].infantry_kind == Some(InfantryKind::Ordinary))
        .unwrap()
        .clone()
}
/// Cases: airlog:50.13, airlog:50.17, airlog:50.2
/// Interpretations: interp:airlog-0014
#[test]
fn real_ordinary_machine_gun_and_heavy_units_hold_one_fire() {
    let (c, s) = setup();
    for (kind, rate) in [
        (InfantryKind::Ordinary, 1),
        (InfantryKind::MachineGun, 2),
        (InfantryKind::HeavyWeapons, 2),
    ] {
        let id = s
            .land
            .units
            .keys()
            .find(|id| {
                let row = &c.units.units[*id];
                if row.infantry_kind != Some(kind) {
                    return false;
                }
                let class = row
                    .class
                    .as_ref()
                    .and_then(|id| c.units.classes.get(id))
                    .unwrap();
                row.infantry_kind == Some(kind)
                    && (class.ca_off.is_some_and(|n| n > 0) || class.ca_def.is_some_and(|n| n > 0))
            })
            .unwrap();
        let strength = toe_strength(&c, &s.land.units[id]).unwrap().get();
        let action = close_assault_ammo_action(&c, id).unwrap();
        assert_eq!(
            ammunition_cost(
                &c,
                AmmoMode::Played,
                action,
                ToeStrengthPoints::new(strength)
            )
            .unwrap()
            .get(),
            rate * strength
        );
        let capacity = ready_ammo_capacity(&c, &s, id).unwrap().get();
        assert!(capacity >= rate * strength && capacity <= 4 * strength);
    }
}
/// Cases: land:4.45, airlog:50.13, airlog:50.17
#[test]
fn understrength_capacity_uses_actual_n_and_unclassified_is_unsupported() {
    let (mut c, mut s) = setup();
    let id = ordinary(&c, &s);
    s.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: 1 });
    assert_eq!(ready_ammo_capacity(&c, &s, &id).unwrap().get(), 1);
    c.units.units.get_mut(&id).unwrap().infantry_kind = None;
    assert_eq!(
        ready_ammo_capacity(&c, &s, &id),
        Err(SupplyError::Unsupported {
            case: "airlog:50.17"
        })
    );
    let hq = s
        .land
        .units
        .keys()
        .find(|id| {
            let row = &c.units.units[*id];
            let cl = row
                .class
                .as_ref()
                .and_then(|id| c.units.classes.get(id))
                .unwrap();
            cl.unit_type == "headquarters" && matches!(s.land.units[*id].toe, Some(Toe::Normal(_)))
        })
        .unwrap();
    assert_eq!(
        ready_ammo_capacity(&c, &s, hq),
        Err(SupplyError::Unsupported {
            case: "airlog:50.17"
        })
    );
}
/// Cases: airlog:50.13, airlog:50.14, airlog:50.17, airlog:50.2
/// Interpretations: interp:airlog-0014
#[test]
fn distinct_weapon_functions_use_maximum_not_sum_or_ca_union() {
    let (mut c, mut s) = setup();
    let id = ordinary(&c, &s);
    let ids: Vec<_> = c.units.weapons.keys().take(2).cloned().collect();
    for (index, key) in ids.iter().enumerate() {
        let w = c.units.weapons.get_mut(key).unwrap();
        w.barrage = Some(0);
        w.anti_armor = Some(0);
        w.aa = Some(0);
        w.ca_off = Some(i32::from(index == 0));
        w.ca_def = Some(i32::from(index == 1));
    }
    s.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(vec![
        WeaponPoints {
            weapon: ids[0].clone(),
            n: 2,
        },
        WeaponPoints {
            weapon: ids[1].clone(),
            n: 3,
        },
    ]));
    assert_eq!(ready_ammo_capacity(&c, &s, &id).unwrap().get(), 6);
    c.units.weapons.get_mut(&ids[0]).unwrap().barrage = Some(1);
    assert_eq!(ready_ammo_capacity(&c, &s, &id).unwrap().get(), 8);
    c.units.weapons.get_mut(&ids[1]).unwrap().barrage = Some(1);
    assert_eq!(ready_ammo_capacity(&c, &s, &id).unwrap().get(), 20);
}
/// Cases: airlog:50.15, airlog:50.17, airlog:53.24, land:3.6
#[test]
fn ready_top_up_debits_only_ammo_enforces_cap_and_survives_recovery() {
    let (c, mut s) = setup();
    let id = ordinary(&c, &s);
    s.land.units.get_mut(&id).unwrap().location = crate::state::Location::Hex {
        hex: "C4020".into(),
    };
    s.land.units.get_mut(&id).unwrap().trucks = Trucks {
        heavy: 1,
        ..Default::default()
    };
    s.logistics
        .unit_supply
        .entry(id.clone())
        .or_default()
        .carried
        .ammo = 30;
    let cap = ready_ammo_capacity(&c, &s, &id).unwrap().get();
    let from = Endpoint::Cargo(id.clone());
    let to = Endpoint::Ready(id.clone());
    distribution::transfer(
        &c,
        &mut s,
        c.units.units[&id].side,
        &from,
        &to,
        Supplies {
            ammo: cap,
            ..Default::default()
        },
        &CargoPacking::default(),
    )
    .unwrap();
    assert_eq!(s.logistics.unit_supply[&id].ready_ammo.get(), cap);
    assert_eq!(s.logistics.unit_supply[&id].carried.ammo, 30 - cap);
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        distribution::transfer(
            &c,
            &mut s,
            c.units.units[&id].side,
            &from,
            &to,
            Supplies {
                ammo: 1,
                ..Default::default()
            },
            &CargoPacking::default()
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let mut b = s.clone();
    b.logistics.unit_supply.get_mut(&id).unwrap().ready_ammo = AmmoPoints::ZERO;
    crate::testkit::assert_indistinguishable(
        &crate::Cna::dev(),
        &c,
        &s,
        &b,
        c.units.units[&id].side.opponent(),
    );
    let side = c.units.units[&id].side;
    let pool = crate::logistics::pools::add_truck_pool(
        &mut s.logistics,
        None,
        side,
        cna_content::scenario::Placement::Hex {
            hex: "C4020".into(),
        },
        Some(crate::state::Location::Hex {
            hex: "C4020".into(),
        }),
        Trucks {
            heavy: 1,
            ..Default::default()
        },
        Supplies {
            ammo: 20,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        distribution::transfer(
            &c,
            &mut s,
            side,
            &Endpoint::Pool(pool),
            &to,
            Supplies {
                ammo: 1,
                ..Default::default()
            },
            &CargoPacking::default()
        )
        .is_err()
    );
    let recovered: State = serde_json::from_value(before).unwrap();
    assert_eq!(recovered.logistics.unit_supply[&id].ready_ammo.get(), cap);
}
/// Cases: airlog:50.17, airlog:53.24, land:3.6
#[test]
fn unknown_ready_capacity_stops_full_but_dev_omits_only_unit() {
    let (mut c, mut s) = setup();
    let id = ordinary(&c, &s);
    c.units.units.get_mut(&id).unwrap().infantry_kind = None;
    s.land.units.get_mut(&id).unwrap().location = crate::state::Location::Hex {
        hex: "C4020".into(),
    };
    s.land.units.get_mut(&id).unwrap().trucks = Trucks {
        heavy: 1,
        ..Default::default()
    };
    s.logistics
        .unit_supply
        .entry(id.clone())
        .or_default()
        .carried
        .ammo = 20;
    let mut rng = CampaignRng::from_seed([4; 32]);
    let mut events = vec![];
    assert!(
        crate::logistics::batches::enter_distribution_with_policy(
            &c,
            &mut s,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            },
            true
        )
        .is_err()
    );
    s.decisions.pending.clear();
    events.clear();
    crate::logistics::batches::enter_distribution_with_policy(
        &c,
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    let ready = serde_json::to_string(&Endpoint::Ready(id.clone())).unwrap();
    for p in &s.decisions.pending {
        assert!(
            !serde_json::to_string(&p.space)
                .unwrap()
                .contains(&json!(ready).to_string())
        );
    }
    assert!(
        events
            .iter()
            .any(|e| format!("{:?}", e.event).contains("ready-ammunition"))
    );
    assert!(
        events
            .iter()
            .all(|e| e.audience != cna_core::visibility::Audience::Public)
    );
}
