use super::*;
use crate::state::State;

fn fixture() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}
fn query(content: &CnaContent, hex: &str) -> Result<u8, EngineError> {
    source_city_fortification_level(content, &HexId::new(hex))
}
fn expect_invariant(content: &CnaContent, hex: &str, detail: &str) {
    assert_eq!(
        query(content, hex),
        Err(invariant(&HexId::new(hex), detail))
    );
}
fn expect_unknown(content: &CnaContent, hex: &str, detail: &str) {
    assert_eq!(
        query(content, hex),
        Err(unsupported(&HexId::new(hex), detail))
    );
}
fn add_unknown(content: &mut CnaContent, id: &str, hex: &str) {
    // Synthetic otherwise valid identity tests mechanics; it authenticates no source city.
    let mut p = content.places.places["city-bardia-c4321"].clone();
    p.id = id.into();
    p.hex_id = hex.into();
    content.places.places.insert(id.into(), p);
}
/// Cases: land:25.12, land:8.37
/// Interpretations: interp:land-0002
#[test]
fn all_nine_published_witnesses_resolve_without_name_identity() {
    let mut c = fixture();
    // Source expectations are independent of the implementation witness table.
    for (id, hex, level) in [
        ("city-alexandria-e3613", "E3613", 3),
        ("city-alexandria-e3714", "E3714", 3),
        ("city-bardia-c4321", "C4321", 2),
        ("city-benghazi-a4827", "A4827", 2),
        ("city-cairo-e1730", "E1730", 3),
        ("city-cairo-e1829", "E1829", 3),
        ("city-cairo-e1830", "E1830", 3),
        ("city-cairo-e1930", "E1930", 3),
        ("city-cairo-e1931", "E1931", 3),
    ] {
        assert_eq!(query(&c, hex), Ok(level));
        let p = c.places.places.get_mut(id).unwrap();
        p.name = "presentation label only".into();
        p.src.reverse();
        p.src.push("land:25.12".into());
        assert_eq!(query(&c, hex), Ok(level));
    }
    for id in ["city-alexandria-e3613", "city-alexandria-e3714"] {
        assert_eq!(c.places.places[id].place_group, None);
    }
}
/// Cases: land:25.12
#[test]
fn absent_future_and_neighbor_identity_never_defaults_to_an_ordinary_city() {
    let mut c = fixture();
    for hex in ["E3713", "C4320", "A4826"] {
        expect_unknown(&c, hex, "missing positive city identity");
    }
    c.places.places.remove("city-alexandria-e3613");
    // Its positive port still does not supply city identity.
    assert!(c.places.at(&"E3613".into()).any(|p| p.kind == "port"));
    expect_unknown(&c, "E3613", "missing positive city identity");
    add_unknown(&mut c, "city-future", "E3613");
    expect_unknown(&c, "E3613", "unreviewed selected city identity city-future");
    // A plausible prefix/group/name and source citation are not authentication.
    let p = c.places.places.get_mut("city-future").unwrap();
    p.name = "Alexandria".into();
    p.place_group = Some("alexandria".into());
    expect_unknown(&c, "E3613", "unreviewed selected city identity city-future");
}
/// Cases: land:25.12
#[test]
fn selected_tuple_and_loader_shape_mutations_are_invariant_for_every_city() {
    let mut c = fixture();
    let original = c.places.clone();
    for w in &WITNESSES {
        for mutation in 0..12 {
            c.places = original.clone();
            let p = c.places.places.get_mut(w.id).unwrap();
            match mutation {
                0 => p.id = "changed.id".into(),
                1 => p.hex_id = "NO_HEX".into(),
                2 => p.hex_id = "C4320".into(),
                3 => p.kind = "port".into(),
                4 => p.place_group = Some("changed.group".into()),
                5 => p.review_batch = "unreviewed".into(),
                6 => p.src.clear(),
                7 => p.src.push(String::new()),
                8 => p.src.retain(|s| s != w.required[0]),
                9 => p.name.clear(),
                10 => p.id.clear(),
                11 => p.review_batch.clear(),
                _ => unreachable!(),
            }
            expect_invariant(
                &c,
                w.hex,
                &format!("malformed selected city witness {}", w.id),
            );
        }
        c.places = original.clone();
        let p = c.places.places.remove(w.id).unwrap();
        c.places.places.insert("renamed.key".into(), p);
        expect_invariant(&c, w.hex, "malformed selected city witness renamed.key");
    }
}
/// Cases: land:25.12
#[test]
fn complete_selection_retains_expected_and_other_recognized_witnesses() {
    let mut c = fixture();
    // Expected key retains both changed kind and moved anchor.
    let p = c.places.places.get_mut("city-bardia-c4321").unwrap();
    p.hex_id = "E3713".into();
    p.kind = "village".into();
    expect_invariant(
        &c,
        "C4321",
        "malformed selected city witness city-bardia-c4321",
    );
    // A different recognized witness moved here is selected even with wrong kind.
    let p = c.places.places.get_mut("city-benghazi-a4827").unwrap();
    p.hex_id = "E1730".into();
    p.kind = "village".into();
    expect_invariant(
        &c,
        "E1730",
        "malformed selected city witness city-benghazi-a4827",
    );
    // When all identity/anchor/kind linkage is erased there is only absent evidence.
    let mut p = c.places.places.remove("city-bardia-c4321").unwrap();
    p.id = "unlinked".into();
    c.places.places.insert(p.id.clone(), p);
    expect_unknown(&c, "C4321", "missing positive city identity");
}
/// Cases: land:25.12
#[test]
fn malformed_precedes_unknown_and_known_with_stable_detail_order() {
    let mut c = fixture();
    add_unknown(&mut c, "aaa.future", "C4321");
    add_unknown(&mut c, "zzz.future", "C4321");
    expect_unknown(&c, "C4321", "unreviewed selected city identity aaa.future");
    // Later expected record corruption outranks earlier unresolved identity.
    c.places
        .places
        .get_mut("city-bardia-c4321")
        .unwrap()
        .src
        .clear();
    expect_invariant(
        &c,
        "C4321",
        "malformed selected city witness city-bardia-c4321",
    );
    // Within the malformed class the first selected key wins, not last-wins.
    c.places.places.get_mut("aaa.future").unwrap().name.clear();
    expect_invariant(&c, "C4321", "malformed selected city witness aaa.future");
    c.places.places.remove("aaa.future");
    c.places.places.remove("city-bardia-c4321");
    c.places.places.get_mut("zzz.future").unwrap().id = "different".into();
    expect_invariant(&c, "C4321", "malformed selected city witness zzz.future");
}
/// Cases: land:25.12
#[test]
fn valid_geometry_aliases_match_and_invalid_geometry_is_not_unknown() {
    let c = fixture();
    // Actual source alias, not an invented city alias. Neither cell is a city witness.
    assert_eq!(c.map.canonical(&"D0200".into()), Some(&HexId::new("C0233")));
    assert_eq!(query(&c, "D0200"), query(&c, "C0233"));
    expect_invariant(&c, "NO_HEX", "requested hex is not existing map geometry");
    expect_invariant(&c, "", "requested hex is not existing map geometry");
}
fn public_content_snapshot(c: &CnaContent) -> String {
    // All public content subtrees, not CnaContent's abbreviated Debug summary.
    format!(
        "{:?}",
        (
            &c.map,
            &c.places,
            &c.areas,
            &c.units,
            &c.scenario,
            &c.tables,
            &c.registry,
            &c.bounds,
            &c.initiative_ratings
        )
    )
}
/// Cases: land:25.12
#[test]
fn unrelated_places_and_current_state_cannot_change_the_source_query() {
    let mut c = fixture();
    let state = State::new(&c).unwrap();
    let state_before = serde_json::to_value(&state).unwrap();
    // An unrelated malformed port is not a selected city witness or a global validator target.
    let p = c.places.places.get_mut("port-bardia-c4321").unwrap();
    p.name.clear();
    p.id = "different".into();
    p.hex_id = "NO_HEX".into();
    let content_before = public_content_snapshot(&c);
    assert_eq!(query(&c, "C4321"), Ok(2));
    expect_unknown(&c, "E3713", "missing positive city identity");
    expect_invariant(&c, "NO_HEX", "requested hex is not existing map geometry");
    assert_eq!(public_content_snapshot(&c), content_before);
    assert_eq!(serde_json::to_value(&state).unwrap(), state_before);
    // Copy an intact witness BEFORE making the independent malformed control.
    add_unknown(&mut c, "future", "E1730");
    c.places
        .places
        .get_mut("city-bardia-c4321")
        .unwrap()
        .src
        .clear();
    let malformed_before = public_content_snapshot(&c);
    expect_invariant(
        &c,
        "C4321",
        "malformed selected city witness city-bardia-c4321",
    );
    assert_eq!(public_content_snapshot(&c), malformed_before);
    let unresolved_before = public_content_snapshot(&c);
    expect_unknown(&c, "E1730", "unreviewed selected city identity future");
    assert_eq!(public_content_snapshot(&c), unresolved_before);
    assert_eq!(serde_json::to_value(&state).unwrap(), state_before);
    // None of these source results adjudicates state initialization or damage.
}
