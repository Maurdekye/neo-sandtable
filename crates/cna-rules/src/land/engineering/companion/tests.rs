use super::*;
use cna_content::units::{
    EngineeringMetadata, EngineeringToeRequirement, NormalToe, Toe, WeaponPoints,
};

const ENGINEER: &str = "it.benghazi_garrison.viii_ii_engineer_bn";
const BENEFITS: [MinefieldCompanionBenefit; 3] = [
    MinefieldCompanionBenefit::EnemyEntryCost,
    MinefieldCompanionBenefit::FriendlyEntryCost,
    MinefieldCompanionBenefit::EnemyVehicleLossProtection,
];
fn fixture() -> (CnaContent, State, UnitId) {
    let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    let id = UnitId::new(ENGINEER);
    install_current(&content, &mut state, &id);
    (content, state, id)
}
// Query-only contexts never grant reinforcement arrival or validate unresolved records.
fn install_current(content: &CnaContent, state: &mut State, id: &UnitId) {
    if !state.land.units.contains_key(id) {
        let mut unit = state.land.units.values().next().unwrap().clone();
        unit.id = id.clone();
        unit.side = content.units.units[id].side;
        unit.toe = content.units.units[id].toe.clone();
        state.land.units.insert(id.clone(), unit);
    }
}
fn row(weapon: &str, n: i32) -> WeaponPoints {
    WeaponPoints {
        weapon: weapon.into(),
        n,
    }
}
fn set_metadata(
    content: &mut CnaContent,
    state: &mut State,
    id: &UnitId,
    original: &EngineeringMetadata,
    scope: EngineeringScope,
    role: EngineeringRole,
    side: Side,
) {
    // BOTH sides are deliberately synthetic here; a mismatch is tested separately.
    let source = content.units.units.get_mut(id).unwrap();
    source.side = side;
    let mut m = original.clone();
    m.scope = scope;
    m.role = (scope != EngineeringScope::None).then_some(role);
    m.toe_requirement =
        (scope == EngineeringScope::AntiMineOnly).then(|| EngineeringToeRequirement {
            weapon: "cw.scorpion".into(),
            min_points: 6,
        });
    source.engineering = Some(m);
    let unit = state.land.units.get_mut(id).unwrap();
    unit.side = side;
    unit.toe = Some(Toe::Weapons(vec![row("cw.scorpion", 6)]));
}
fn query(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    benefit: MinefieldCompanionBenefit,
) -> Result<bool, EngineError> {
    let before = serde_json::to_value(state).unwrap();
    let units = format!("{:?}", content.units);
    let source = content.units.units.get(id).cloned();
    let result = minefield_companion_qualifies(content, state, id, benefit);
    assert_eq!(serde_json::to_value(state).unwrap(), before);
    assert_eq!(format!("{:?}", content.units), units);
    assert_eq!(content.units.units.get(id).cloned(), source);
    result
}
fn all_results(content: &CnaContent, state: &State, id: &UnitId, expected: [bool; 3]) {
    for (benefit, expected) in BENEFITS.into_iter().zip(expected) {
        assert_eq!(query(content, state, id, benefit), Ok(expected));
    }
}
fn raw_error_first(content: &CnaContent, state: &State, id: &UnitId) {
    let error = source_capability(content, state, id).unwrap_err();
    for benefit in BENEFITS {
        assert_eq!(query(content, state, id, benefit), Err(error.clone()));
    }
}

/// Cases: land:23.13, land:23.14, land:23.15, land:26.24, land:26.25
/// Interpretations: interp:land-0037
#[test]
fn every_scope_role_and_side_has_separate_companion_benefits() {
    let (mut content, mut state, id) = fixture();
    let original = content.units.units[&id].engineering.clone().unwrap();
    for scope in [
        EngineeringScope::General,
        EngineeringScope::RailroadOnly,
        EngineeringScope::RoadOnly,
        EngineeringScope::AntiMineOnly,
        EngineeringScope::None,
    ] {
        for role in [
            EngineeringRole::Company,
            EngineeringRole::Battalion,
            EngineeringRole::Headquarters,
        ] {
            for side in Side::ALL {
                set_metadata(&mut content, &mut state, &id, &original, scope, role, side);
                let expected = match (scope, role, side) {
                    (EngineeringScope::General, EngineeringRole::Battalion, _) => [true; 3],
                    (EngineeringScope::AntiMineOnly, EngineeringRole::Battalion, _) => [true; 3],
                    (
                        EngineeringScope::General,
                        EngineeringRole::Headquarters,
                        Side::Commonwealth,
                    ) => [true, false, true],
                    _ => [false; 3],
                };
                all_results(&content, &state, &id, expected);
            }
        }
    }
}
/// Cases: land:23.13, land:24.61, land:26.24, land:26.25
/// Interpretations: interp:land-0037
#[test]
fn actual_source_rows_keep_rail_company_and_printed_echelon_distinct() {
    let (content, mut state, _) = fixture();
    for (name, expected) in [
        (ENGINEER, [true; 3]),
        ("cw.unassigned_nz.10th_nz_rr_construction_coy", [false; 3]),
        ("cw.unassigned_nz.13th_nz_rr_construction_coy", [false; 3]),
    ] {
        let id = UnitId::new(name);
        install_current(&content, &mut state, &id);
        all_results(&content, &state, &id, expected);
        assert_eq!(
            content.units.units[&id].echelon.as_deref(),
            Some("battalion")
        );
    }
}
/// Cases: land:23.15, land:26.24, land:26.25
/// Interpretations: interp:land-0037
#[test]
fn current_scorpion_gate_and_duplicates_control_all_benefits() {
    let (mut content, mut state, id) = fixture();
    let original = content.units.units[&id].engineering.clone().unwrap();
    set_metadata(
        &mut content,
        &mut state,
        &id,
        &original,
        EngineeringScope::AntiMineOnly,
        EngineeringRole::Battalion,
        Side::Commonwealth,
    );
    // A former/source TOE cannot replace current composition.
    content.units.units.get_mut(&id).unwrap().toe =
        Some(Toe::Weapons(vec![row("cw.scorpion", 100)]));
    for (rows, eligible) in [
        (vec![], false),
        (vec![row("cw.mk_vi_light", 20)], false),
        (vec![row("cw.scorpion", 5)], false),
        (vec![row("cw.scorpion", 6)], true),
        (vec![row("cw.scorpion", 7)], true),
        (vec![row("cw.scorpion", 3), row("cw.scorpion", 3)], true),
    ] {
        state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(rows));
        all_results(&content, &state, &id, [eligible; 3]);
    }
}
/// Cases: land:23.15, land:26.24, land:26.25
/// Interpretations: interp:land-0037
#[test]
fn side_guard_precedes_every_known_negative_but_is_query_local() {
    let (mut content, mut state, id) = fixture();
    let original = content.units.units[&id].engineering.clone().unwrap();
    for (scope, role, points) in [
        (EngineeringScope::None, EngineeringRole::Company, 6),
        (EngineeringScope::General, EngineeringRole::Company, 6),
        (EngineeringScope::RoadOnly, EngineeringRole::Battalion, 6),
        (EngineeringScope::RailroadOnly, EngineeringRole::Company, 6),
        (
            EngineeringScope::AntiMineOnly,
            EngineeringRole::Battalion,
            5,
        ),
    ] {
        set_metadata(
            &mut content,
            &mut state,
            &id,
            &original,
            scope,
            role,
            Side::Commonwealth,
        );
        state.land.units.get_mut(&id).unwrap().toe =
            Some(Toe::Weapons(vec![row("cw.scorpion", points)]));
        all_results(&content, &state, &id, [false; 3]);
        state.land.units.get_mut(&id).unwrap().side = Side::Axis;
        // Published raw capability is UNCHANGED and does not enforce this guard.
        assert!(source_capability(&content, &state, &id).is_ok());
        for benefit in BENEFITS {
            assert_eq!(
                query(&content, &state, &id, benefit),
                Err(EngineError::Invariant {
                    detail: format!("engineering companion {id}: runtime/content side mismatch"),
                })
            );
        }
    }
}
/// Cases: land:23.11, land:23.15, land:26.24, land:26.25
/// Interpretations: interp:land-0037
#[test]
fn raw_gaps_and_corruption_win_over_side_and_role_negatives() {
    let (mut content, mut state, id) = fixture();
    let original = content.units.units[&id].engineering.clone().unwrap();
    // Corrupt side is deliberate. No wrapper may replace the raw error.
    state.land.units.get_mut(&id).unwrap().side = Side::Commonwealth;
    content.units.units.get_mut(&id).unwrap().engineering = None;
    raw_error_first(&content, &state, &id);
    for problem in 0..8 {
        set_metadata(
            &mut content,
            &mut state,
            &id,
            &original,
            EngineeringScope::AntiMineOnly,
            EngineeringRole::Company,
            Side::Commonwealth,
        );
        state.land.units.get_mut(&id).unwrap().side = Side::Axis;
        match problem {
            0 => {
                content
                    .units
                    .units
                    .get_mut(&id)
                    .unwrap()
                    .engineering
                    .as_mut()
                    .unwrap()
                    .toe_requirement = None
            }
            1 => {
                content
                    .units
                    .units
                    .get_mut(&id)
                    .unwrap()
                    .engineering
                    .as_mut()
                    .unwrap()
                    .toe_requirement
                    .as_mut()
                    .unwrap()
                    .min_points = 5
            }
            2 => {
                content
                    .units
                    .units
                    .get_mut(&id)
                    .unwrap()
                    .engineering
                    .as_mut()
                    .unwrap()
                    .role = None
            }
            3 => state.land.units.get_mut(&id).unwrap().toe = None,
            4 => state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Normal(NormalToe::N)),
            5 => state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: 1 }),
            6 => state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Over { over: 1 }),
            _ => {
                content
                    .units
                    .units
                    .get_mut(&id)
                    .unwrap()
                    .engineering
                    .as_mut()
                    .unwrap()
                    .evidence
                    .verification = "single".into()
            }
        }
        raw_error_first(&content, &state, &id);
    }
    for first in [0, 5, 6] {
        for bad in [
            row("unknown.weapon", 0),
            row("cw.mk_vi_light", -1),
            row("cw.scorpion", -1),
        ] {
            set_metadata(
                &mut content,
                &mut state,
                &id,
                &original,
                EngineeringScope::AntiMineOnly,
                EngineeringRole::Company,
                Side::Commonwealth,
            );
            state.land.units.get_mut(&id).unwrap().side = Side::Axis;
            state.land.units.get_mut(&id).unwrap().toe =
                Some(Toe::Weapons(vec![row("cw.scorpion", first), bad]));
            raw_error_first(&content, &state, &id);
        }
    }
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(vec![
        row("cw.scorpion", i32::MAX),
        row("cw.scorpion", 1),
    ]));
    raw_error_first(&content, &state, &id);
    // Contradictory verified-none still fails raw metadata before side guard.
    let m = content
        .units
        .units
        .get_mut(&id)
        .unwrap()
        .engineering
        .as_mut()
        .unwrap();
    m.scope = EngineeringScope::None;
    raw_error_first(&content, &state, &id);
}
/// Cases: land:23.11, land:26.24, land:26.25
/// Interpretations: interp:land-0037
#[test]
fn trusted_missing_and_stored_identity_errors_remain_exact() {
    let (content, state, id) = fixture();
    let mut s = state.clone();
    s.land.units.remove(&id);
    raw_error_first(&content, &s, &id);
    let mut c = fixture().0;
    c.units.units.remove(&id);
    raw_error_first(&c, &state, &id);
    for runtime in [true, false] {
        let mut c = fixture().0;
        let mut s = state.clone();
        c.units.units.get_mut(&id).unwrap().engineering = None;
        s.land.units.get_mut(&id).unwrap().side = Side::Commonwealth;
        if runtime {
            s.land.units.get_mut(&id).unwrap().id = "different.id".into();
        } else {
            c.units.units.get_mut(&id).unwrap().id = "different.id".into();
        }
        raw_error_first(&c, &s, &id);
    }
}
