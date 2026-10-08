use super::*;
use cna_content::units::{EngineeringToeRequirement, NormalToe, WeaponPoints};

const ENGINEER: &str = "it.benghazi_garrison.viii_ii_engineer_bn";
fn fixture() -> (CnaContent, State, UnitId) {
    let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    let id = UnitId::new(ENGINEER);
    install_current(&content, &mut state, &id);
    (content, state, id)
}
// Synthetic current-unit context tests source queries, never arrival permissions.
fn install_current(content: &CnaContent, state: &mut State, id: &UnitId) {
    if !state.land.units.contains_key(id) {
        let mut unit = state.land.units.values().next().unwrap().clone();
        unit.id = id.clone();
        unit.side = content.units.units[id].side;
        unit.toe = content.units.units[id].toe.clone();
        state.land.units.insert(id.clone(), unit);
    }
}
fn metadata<'a>(content: &'a mut CnaContent, id: &UnitId) -> &'a mut EngineeringMetadata {
    content
        .units
        .units
        .get_mut(id)
        .unwrap()
        .engineering
        .as_mut()
        .unwrap()
}
fn scorpion(content: &mut CnaContent, id: &UnitId) {
    let metadata = metadata(content, id);
    metadata.scope = EngineeringScope::AntiMineOnly;
    metadata.src = vec!["land:23.15".into()];
    metadata.toe_requirement = Some(EngineeringToeRequirement {
        weapon: "cw.scorpion".into(),
        min_points: 6,
    });
}
fn row(weapon: &str, n: i32) -> WeaponPoints {
    WeaponPoints {
        weapon: weapon.into(),
        n,
    }
}
fn query(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<Option<EngineerCapability>, EngineError> {
    let before = serde_json::to_value(state).unwrap();
    let source = format!("{:?}", content.units.units);
    let result = source_capability(content, state, id);
    assert_eq!(serde_json::to_value(state).unwrap(), before);
    assert_eq!(format!("{:?}", content.units.units), source);
    result
}
fn expect_invariant(
    result: Result<Option<EngineerCapability>, EngineError>,
    id: &UnitId,
    detail: &str,
) {
    assert!(
        matches!(result, Err(EngineError::Invariant {detail: actual}) if actual == format!("engineering capability {id}: {detail}"))
    );
}
fn expect_gap(
    result: Result<Option<EngineerCapability>, EngineError>,
    id: &UnitId,
    case: &str,
    detail: &str,
) {
    assert!(
        matches!(result, Err(EngineError::Unsupported {case: actual_case, detail: actual}) if actual_case == case && actual == format!("engineering capability {id}: {detail}"))
    );
}

/// Cases: land:23.11, land:23.13, land:24.61
#[test]
fn verified_source_rows_keep_scope_and_procedure_role_without_changing_echelon() {
    let (content, mut state, _) = fixture();
    for (raw, scope, role) in [
        (
            ENGINEER,
            EngineeringScope::General,
            EngineeringRole::Battalion,
        ),
        (
            "cw.unassigned_nz.10th_nz_rr_construction_coy",
            EngineeringScope::RailroadOnly,
            EngineeringRole::Company,
        ),
        (
            "cw.unassigned_nz.13th_nz_rr_construction_coy",
            EngineeringScope::RailroadOnly,
            EngineeringRole::Company,
        ),
    ] {
        let id = UnitId::new(raw);
        install_current(&content, &mut state, &id);
        assert_eq!(
            query(&content, &state, &id).unwrap(),
            Some(EngineerCapability { scope, role })
        );
        assert_eq!(
            content.units.units[&id].echelon.as_deref(),
            Some("battalion")
        );
    }
}
/// Cases: land:23.11, land:23.14
#[test]
fn same_class_and_legacy_hq_flags_cannot_replace_unresolved_source() {
    let (content, mut state, id) = fixture();
    let mg = UnitId::new("it.1ccnn_div.201st_machinegun_bn");
    assert_eq!(
        content.units.units[&id].class,
        content.units.units[&mg].class
    );
    let hq = UnitId::new("cw.7_armd_div.7th_armored_div_hq");
    assert!(content.units.units[&hq].engineer_hq);
    for id in [mg, hq] {
        install_current(&content, &mut state, &id);
        expect_gap(
            query(&content, &state, &id),
            &id,
            "land:23.11",
            "source-backed engineering identity is unresolved",
        );
    }
}
/// Cases: land:23.11, land:23.15
#[test]
fn complete_metadata_is_required_even_before_a_verified_negative() {
    let (valid, state, id) = fixture();
    let mut content = fixture().0;
    let m = metadata(&mut content, &id);
    m.scope = EngineeringScope::None;
    m.role = None;
    assert_eq!(query(&content, &state, &id).unwrap(), None);
    for (field, detail) in [
        (0, "missing engineering source citations"),
        (1, "invalid engineering source evidence"),
        (2, "contradictory verified non-engineer metadata"),
        (3, "contradictory verified non-engineer metadata"),
        (4, "invalid engineering source evidence"),
        (5, "invalid engineering source evidence"),
        (6, "missing engineering source citations"),
    ] {
        let mut c = fixture().0;
        c.units.units.get_mut(&id).unwrap().engineering =
            content.units.units[&id].engineering.clone();
        let m = metadata(&mut c, &id);
        match field {
            0 => m.src = vec![" ".into()],
            1 => m.evidence.verification = "single".into(),
            2 => m.role = Some(EngineeringRole::Company),
            3 => {
                m.toe_requirement = Some(EngineeringToeRequirement {
                    weapon: "cw.scorpion".into(),
                    min_points: 6,
                })
            }
            4 => m.evidence.transcribed_from.truncate(1),
            5 => m.evidence.transcribed_from = vec!["oa".into(), " ".into()],
            _ => m.src.clear(),
        }
        expect_invariant(query(&c, &state, &id), &id, detail);
    }
    let mut c = valid;
    metadata(&mut c, &id).role = None;
    expect_invariant(
        query(&c, &state, &id),
        &id,
        "positive engineering scope has no role",
    );
}
/// Cases: land:23.11
#[test]
fn trusted_missing_or_mismatched_ids_are_invariants_before_source_results() {
    let (_, state, id) = fixture();
    let mut c = fixture().0;
    let mut s = state.clone();
    s.land.units.remove(&id);
    expect_invariant(query(&c, &s, &id), &id, "missing trusted runtime unit");
    c.units.units.remove(&id);
    expect_invariant(query(&c, &state, &id), &id, "missing trusted content unit");
    for runtime in [true, false] {
        let mut c = fixture().0;
        let mut s = state.clone();
        // Also omit metadata: a mismatched stored ID must not be labelled a source gap.
        c.units.units.get_mut(&id).unwrap().engineering = None;
        if runtime {
            s.land.units.get_mut(&id).unwrap().id = "different.id".into();
        } else {
            c.units.units.get_mut(&id).unwrap().id = "different.id".into();
        }
        expect_invariant(
            query(&c, &s, &id),
            &id,
            "stored runtime/content identity mismatch",
        );
    }
}
/// Cases: land:23.15
#[test]
fn actual_current_scorpion_points_include_duplicates_and_known_absence() {
    let (mut content, mut state, id) = fixture();
    scorpion(&mut content, &id);
    for (rows, eligible) in [
        (vec![], false),
        (vec![row("cw.mk_vi_light", 20)], false),
        (vec![row("cw.scorpion", 5)], false),
        (vec![row("cw.scorpion", 6)], true),
        (vec![row("cw.scorpion", 7)], true),
        (vec![row("cw.scorpion", 3), row("cw.scorpion", 3)], true),
    ] {
        state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(rows));
        let expected = eligible.then_some(EngineerCapability {
            scope: EngineeringScope::AntiMineOnly,
            role: EngineeringRole::Battalion,
        });
        assert_eq!(query(&content, &state, &id).unwrap(), expected);
    }
}
/// Cases: land:23.15
#[test]
fn omitted_or_symbolic_current_toe_does_not_use_source_toe() {
    let (mut content, mut state, id) = fixture();
    scorpion(&mut content, &id);
    content.units.units.get_mut(&id).unwrap().toe =
        Some(Toe::Weapons(vec![row("cw.scorpion", 100)]));
    for toe in [
        None,
        Some(Toe::Normal(NormalToe::N)),
        Some(Toe::Under { under: 1 }),
        Some(Toe::Over { over: 2 }),
    ] {
        state.land.units.get_mut(&id).unwrap().toe = toe;
        expect_gap(
            query(&content, &state, &id),
            &id,
            "land:23.15",
            "current identified equipment is unresolved",
        );
    }
}
/// Cases: land:23.15
#[test]
fn missing_gate_is_a_gap_but_present_invalid_gates_are_invariants() {
    let (mut content, state, id) = fixture();
    scorpion(&mut content, &id);
    metadata(&mut content, &id).toe_requirement = None;
    expect_gap(
        query(&content, &state, &id),
        &id,
        "land:23.15",
        "engineering equipment gate is unresolved",
    );
    for (scope, weapon, min_points) in [
        (EngineeringScope::General, "cw.scorpion", 6),
        (EngineeringScope::AntiMineOnly, "unknown.weapon", 6),
        (EngineeringScope::AntiMineOnly, "cw.mk_vi_light", 6),
        (EngineeringScope::AntiMineOnly, "cw.scorpion", 5),
        (EngineeringScope::AntiMineOnly, "cw.scorpion", -1),
    ] {
        let mut c = fixture().0;
        let m = metadata(&mut c, &id);
        m.scope = scope;
        m.toe_requirement = Some(EngineeringToeRequirement {
            weapon: weapon.into(),
            min_points,
        });
        expect_invariant(query(&c, &state, &id), &id, "corrupt engineering TOE gate");
    }
    let mut c = content;
    scorpion(&mut c, &id);
    c.units.weapons.remove("cw.scorpion");
    expect_invariant(query(&c, &state, &id), &id, "corrupt engineering TOE gate");
}
/// Cases: land:23.15
#[test]
fn every_late_or_nonmatching_row_is_validated_before_threshold_or_zero() {
    let (mut content, mut state, id) = fixture();
    scorpion(&mut content, &id);
    for first in [0, 5, 6] {
        for invalid in [
            row("unknown.weapon", 0),
            row("cw.mk_vi_light", -1),
            row("cw.scorpion", -1),
        ] {
            state.land.units.get_mut(&id).unwrap().toe =
                Some(Toe::Weapons(vec![row("cw.scorpion", first), invalid]));
            expect_invariant(
                query(&content, &state, &id),
                &id,
                "invalid current equipment row",
            );
        }
    }
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(vec![
        row("cw.scorpion", i32::MAX),
        row("cw.scorpion", 1),
    ]));
    expect_invariant(
        query(&content, &state, &id),
        &id,
        "current engineering weapon points overflow",
    );
}
