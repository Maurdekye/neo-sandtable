//! Loading, envelope validation and rule-registry references.

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use cna_tables::raw::read_all;
use cna_tables::validate::{check_envelopes, check_rule_references};
use cna_tables::{RawTable, Tables};
use common::{data_dir, tables};

#[test]
fn every_table_file_loads_and_binds() {
    let t = tables();
    assert!(!Tables::bound_ids().is_empty());
    let _ = &t.airlog;
}

/// Table ids that have a file but no typed binding yet. Empty means every table is bound; a new
/// table file that is not bound fails `every_table_file_has_a_binding`.
const PENDING_BINDINGS: &[&str] = &[
    "land.27.92.raid_on_rommel",
    "land.27.93.sas_brigade_raid",
    "land.30.46.chariot_raid",
    "land.30.6.cw_fleet_reinforcement",
    "land.32.46.axis_supply_availability",
    "land.32.47.cw_supply_availability",
    "land.32.66.axis_convoy_bombing",
    "land.16.6.patrol_survival",
    "land.16.7.patrol_recon",
    "land.16.8.objective_loss",
    "land.19.5.maximum_attachment",
    "land.20.66.axis_replacement_pool",
    "land.20.78.commonwealth_production",
];

#[test]
fn every_table_file_has_a_binding() {
    let files: BTreeSet<String> = read_all(&data_dir())
        .expect("read")
        .into_iter()
        .map(|t| t.envelope.id)
        .collect();
    let bound: BTreeSet<String> = Tables::bound_ids().iter().map(|s| s.to_string()).collect();
    let pending: BTreeSet<String> = PENDING_BINDINGS.iter().map(|s| s.to_string()).collect();
    let unbound: BTreeSet<_> = files
        .difference(&bound)
        .filter(|id| !pending.contains(*id))
        .collect();
    assert!(
        unbound.is_empty(),
        "table files with no typed binding in cna-tables: {unbound:?}"
    );
    let stale: BTreeSet<_> = pending.intersection(&bound).collect();
    assert!(
        stale.is_empty(),
        "PENDING_BINDINGS lists bound tables: {stale:?}"
    );
    let ghosts: BTreeSet<_> = bound.difference(&files).collect();
    assert!(
        ghosts.is_empty(),
        "bindings whose table file is missing: {ghosts:?}"
    );
}

#[test]
fn rule_registry_references_resolve() {
    let ids: BTreeSet<String> = read_all(&data_dir())
        .expect("read")
        .into_iter()
        .map(|t| t.envelope.id)
        .collect();
    check_rule_references(&data_dir(), &ids).expect("every referenced table exists");
}

#[test]
fn the_real_envelopes_are_valid() {
    let all = read_all(&data_dir()).expect("read");
    check_envelopes(&all).expect("envelopes");
}

// ---- known-negative fixtures: each check must fail on a deliberately broken input ----

fn parse(path: &str, text: &str) -> RawTable {
    RawTable::parse(Path::new(path), text).expect("fixture parses")
}

const GOOD: &str = r#"
[table]
id = "airlog.1.1.sample"
case = "1.1"
title = "Sample"
src = ["airlog:1.1"]
dice = "none"
"#;

#[test]
fn broken_toml_names_the_file() {
    let err = RawTable::parse(Path::new("data/tables/airlog/1.1-a.toml"), "this = [").unwrap_err();
    assert!(err.to_string().contains("1.1-a.toml"), "{err}");
}

#[test]
fn missing_envelope_names_the_field() {
    let err = RawTable::parse(Path::new("data/tables/airlog/1.1-a.toml"), "x = 1").unwrap_err();
    assert_eq!(err.field, "table");
    let no_dice = GOOD.replace("dice = \"none\"", "");
    let err = RawTable::parse(Path::new("data/tables/airlog/1.1-a.toml"), &no_dice).unwrap_err();
    assert_eq!(err.field, "table.dice");
}

#[test]
fn duplicate_ids_are_rejected() {
    let a = parse("data/tables/airlog/1.1-a.toml", GOOD);
    let b = parse("data/tables/airlog/1.1-b.toml", GOOD);
    let err = check_envelopes(&[a, b]).unwrap_err();
    assert_eq!(err.field, "table.id");
    assert!(err.message.contains("already used"), "{err}");
}

#[test]
fn an_id_must_match_its_book_directory_and_case() {
    let wrong_book = parse("data/tables/land/1.1-a.toml", GOOD);
    assert!(check_envelopes(&[wrong_book]).is_err());
    let wrong_case = GOOD.replace("id = \"airlog.1.1.sample\"", "id = \"airlog.2.2.sample\"");
    assert!(check_envelopes(&[parse("data/tables/airlog/1.1-a.toml", &wrong_case)]).is_err());
    let no_slug = GOOD.replace("airlog.1.1.sample", "airlog.1.1.");
    assert!(check_envelopes(&[parse("data/tables/airlog/1.1-a.toml", &no_slug)]).is_err());
}

#[test]
fn a_file_name_must_start_with_its_case() {
    let t = parse("data/tables/airlog/9.9-a.toml", GOOD);
    let err = check_envelopes(&[t]).unwrap_err();
    assert_eq!(err.field, "table.case");
}

#[test]
fn a_table_must_cite_a_source() {
    let t = parse(
        "data/tables/airlog/1.1-a.toml",
        &GOOD.replace("[\"airlog:1.1\"]", "[]"),
    );
    let err = check_envelopes(&[t]).unwrap_err();
    assert_eq!(err.field, "table.src");
}

#[test]
fn a_rule_referencing_a_missing_table_is_rejected() {
    let dir = std::env::temp_dir().join(format!("cna-tables-fixture-{}", std::process::id()));
    let rules = dir.join("rules").join("airlog");
    std::fs::create_dir_all(&rules).expect("mkdir");
    std::fs::write(
        rules.join("01-x.toml"),
        "[[case]]\nid = \"1.1\"\ntables = [\"airlog.9.9.nope\"]\n",
    )
    .expect("write");
    let known: BTreeSet<String> = BTreeSet::from(["airlog.1.1.sample".to_string()]);
    let err = check_rule_references(&dir, &known).unwrap_err();
    assert!(err.message.contains("airlog.9.9.nope"), "{err}");
    assert_eq!(err.field, "case 1.1.tables");
    std::fs::remove_dir_all(&dir).ok();
}
