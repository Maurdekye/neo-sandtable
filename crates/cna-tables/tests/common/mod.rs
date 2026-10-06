//! Shared helpers for the integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cna_tables::{Bound, RawTable, TableError, Tables};

/// The repository `data/` directory.
pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

/// All tables, loaded once from the real data.
pub fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables::load(&data_dir()).unwrap_or_else(|e| panic!("{e}")))
}

/// The text of one real table file, found by case-and-slug prefix, e.g. `airlog/49.19-`.
pub fn table_text(prefix: &str) -> (PathBuf, String) {
    let (book, stem) = prefix.split_once('/').expect("book/prefix");
    let dir = data_dir().join("tables").join(book);
    let hit = std::fs::read_dir(&dir)
        .expect("table dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(stem))
        })
        .unwrap_or_else(|| panic!("no table file starts with {prefix}"));
    let text = std::fs::read_to_string(&hit).expect("read");
    (hit, text)
}

/// Bind a table from file text altered by `edit`; used by the known-negative tests.
pub fn bind_edited<T: Bound>(
    prefix: &str,
    edit: impl FnOnce(&str) -> String,
) -> Result<T, TableError> {
    let (path, text) = table_text(prefix);
    let edited = edit(&text);
    assert_ne!(edited, text, "the edit changed nothing");
    let raw = RawTable::parse(&path, &edited)?;
    T::from_raw(&raw)
}

/// Replace the first occurrence of `from` (which must exist) with `to`.
pub fn replace_once(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "fixture text `{from}` not found");
    text.replacen(from, to, 1)
}
