//! Checks that apply to the whole set of table files and to the rule registry's references.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::error::TableError;
use crate::raw::{RawTable, list_toml_files};

/// Envelope-level checks across every table file:
/// - every `[table].id` is unique;
/// - the id reads `<book>.<case>.<slug>`, where `<book>` is the directory the file sits in and
///   `<case>` equals `[table].case`;
/// - the file name starts with `<case>-`;
/// - the table cites at least one source case.
pub fn check_envelopes(tables: &[RawTable]) -> Result<(), TableError> {
    let mut seen: BTreeMap<&str, &RawTable> = BTreeMap::new();
    for t in tables {
        let env = &t.envelope;
        if let Some(prev) = seen.insert(env.id.as_str(), t) {
            return Err(t.err(
                "table.id",
                format!("id `{}` is already used by {}", env.id, prev.path.display()),
            ));
        }
        let prefix = format!("{}.{}.", t.book.dir_name(), env.case);
        if !env.id.starts_with(&prefix) || env.id.len() == prefix.len() {
            return Err(t.err(
                "table.id",
                format!(
                    "id `{}` must read `{prefix}<slug>` (book directory and table.case)",
                    env.id
                ),
            ));
        }
        let stem = t.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if !stem.starts_with(&format!("{}-", env.case)) {
            return Err(t.err(
                "table.case",
                format!("file name `{stem}` must start with `{}-`", env.case),
            ));
        }
        if env.src.is_empty() {
            return Err(t.err("table.src", "a table must cite at least one source case"));
        }
    }
    Ok(())
}

/// Every table id that `[[case]]` records under `<data_dir>/rules` reference through
/// `tables = [...]`, paired with the case that references it.
pub fn rule_table_references(data_dir: &Path) -> Result<Vec<(String, String, String)>, TableError> {
    let root = data_dir.join("rules");
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for path in list_toml_files(&root)? {
        let text = fs::read_to_string(&path)
            .map_err(|e| TableError::file(&path, format!("cannot read file: {e}")))?;
        let doc: toml::Table =
            toml::from_str(&text).map_err(|e| TableError::file(&path, e.message().to_string()))?;
        let Some(toml::Value::Array(cases)) = doc.get("case") else {
            continue;
        };
        for case in cases {
            let Some(case) = case.as_table() else {
                continue;
            };
            let case_id = case.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let Some(toml::Value::Array(refs)) = case.get("tables") else {
                continue;
            };
            for r in refs {
                let Some(id) = r.as_str() else {
                    return Err(TableError::new(
                        &path,
                        format!("case {case_id}.tables"),
                        "table references must be strings",
                    ));
                };
                out.push((
                    id.to_string(),
                    case_id.to_string(),
                    path.display().to_string(),
                ));
            }
        }
    }
    Ok(out)
}

/// Check that every table id the rule registry references exists among `known`.
pub fn check_rule_references(data_dir: &Path, known: &BTreeSet<String>) -> Result<(), TableError> {
    for (id, case, file) in rule_table_references(data_dir)? {
        if !known.contains(&id) {
            return Err(TableError::new(
                Path::new(&file),
                format!("case {case}.tables"),
                format!("references table `{id}`, which has no file under data/tables"),
            ));
        }
    }
    Ok(())
}
