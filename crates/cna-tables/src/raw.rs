//! Reading table files: the common `[table]` envelope and the untyped body.

use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::error::TableError;

/// The books that own tables, named by the directory under `data/tables/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Book {
    Land,
    Airlog,
    Scen,
}

impl Book {
    pub fn dir_name(self) -> &'static str {
        match self {
            Book::Land => "land",
            Book::Airlog => "airlog",
            Book::Scen => "scen",
        }
    }

    fn from_dir(name: &str) -> Option<Self> {
        match name {
            "land" => Some(Book::Land),
            "airlog" => Some(Book::Airlog),
            "scen" => Some(Book::Scen),
            _ => None,
        }
    }
}

/// The dice a table is rolled with (`dice` in the envelope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiceKind {
    None,
    OneD6,
    TwoD6Sum,
    TwoD6Reading,
    Other,
}

/// The keys of the `[table]` envelope itself; any other key under `[table]` belongs to the body.
const ENVELOPE_KEYS: &[&str] = &[
    "id",
    "case",
    "title",
    "src",
    "transcribed_from",
    "verification",
    "dice",
    "notes",
];

/// The `[table]` header every file carries (see `data/tables/README.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub id: String,
    pub case: String,
    pub title: String,
    pub src: Vec<String>,
    pub dice: DiceKind,
}

/// One table file: its envelope and the rest of the file, still untyped.
#[derive(Debug, Clone)]
pub struct RawTable {
    pub path: PathBuf,
    pub book: Book,
    pub envelope: Envelope,
    /// The whole file except the `[table]` header.
    pub body: toml::Table,
}

impl RawTable {
    /// Parse the file text. `path` is used for error messages and to find the owning book.
    pub fn parse(path: &Path, text: &str) -> Result<Self, TableError> {
        let mut file: toml::Table =
            toml::from_str(text).map_err(|e| TableError::file(path, e.message().to_string()))?;
        let book = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .and_then(Book::from_dir)
            .ok_or_else(|| {
                TableError::file(
                    path,
                    "table files must sit directly in land/, airlog/ or scen/",
                )
            })?;
        let header = match file.remove("table") {
            Some(toml::Value::Table(t)) => t,
            Some(_) => return Err(TableError::new(path, "table", "must be a table")),
            None => return Err(TableError::new(path, "table", "missing [table] envelope")),
        };
        let text_field = |key: &str| -> Result<String, TableError> {
            match header.get(key) {
                Some(toml::Value::String(s)) => Ok(s.clone()),
                Some(_) => Err(TableError::new(
                    path,
                    format!("table.{key}"),
                    "must be a string",
                )),
                None => Err(TableError::new(path, format!("table.{key}"), "missing")),
            }
        };
        let src = match header.get("src") {
            Some(toml::Value::Array(items)) => items
                .iter()
                .map(|v| match v {
                    toml::Value::String(s) => Ok(s.clone()),
                    _ => Err(TableError::new(
                        path,
                        "table.src",
                        "entries must be strings",
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(TableError::new(path, "table.src", "must be an array")),
            None => Vec::new(),
        };
        let dice = match header.get("dice") {
            Some(toml::Value::String(s)) => match s.as_str() {
                "none" => DiceKind::None,
                "1d6" => DiceKind::OneD6,
                "2d6_sum" => DiceKind::TwoD6Sum,
                "2d6_reading" => DiceKind::TwoD6Reading,
                "other" => DiceKind::Other,
                other => {
                    return Err(TableError::new(
                        path,
                        "table.dice",
                        format!("unknown dice kind `{other}`"),
                    ));
                }
            },
            _ => {
                return Err(TableError::new(
                    path,
                    "table.dice",
                    "missing or not a string",
                ));
            }
        };
        let envelope = Envelope {
            id: text_field("id")?,
            case: text_field("case")?,
            title: text_field("title")?,
            src,
            dice,
        };
        // Keys written under `[table]` after the envelope fields (such as `places = [...]`) are
        // part of the body, alongside the tables that follow.
        let mut body = file;
        for (key, value) in header {
            if ENVELOPE_KEYS.contains(&key.as_str()) {
                continue;
            }
            if body.insert(key.clone(), value).is_some() {
                return Err(TableError::new(
                    path,
                    format!("table.{key}"),
                    "also defined as a top-level key",
                ));
            }
        }
        Ok(Self {
            path: path.to_path_buf(),
            book,
            envelope,
            body,
        })
    }

    /// Deserialize the body into a typed record; errors name the offending field.
    pub fn deserialize<T: DeserializeOwned>(&self) -> Result<T, TableError> {
        serde_path_to_error::deserialize(toml::Value::Table(self.body.clone())).map_err(|e| {
            TableError::new(
                &self.path,
                e.path().to_string(),
                e.inner().message().to_string(),
            )
        })
    }

    /// An error about a field of this table.
    pub fn err(&self, field: impl Into<String>, message: impl Into<String>) -> TableError {
        TableError::new(&self.path, field, message)
    }
}

/// Every `*.toml` under `root`, sorted by path so loading is deterministic.
pub fn list_toml_files(root: &Path) -> Result<Vec<PathBuf>, TableError> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), TableError> {
        let entries = fs::read_dir(dir)
            .map_err(|e| TableError::file(dir, format!("cannot read directory: {e}")))?;
        let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                walk(&p, out)?;
            } else if p.extension().and_then(|e| e.to_str()) == Some("toml") {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, &mut out)?;
    Ok(out)
}

/// Read every table file under `<data_dir>/tables`.
pub fn read_all(data_dir: &Path) -> Result<Vec<RawTable>, TableError> {
    let root = data_dir.join("tables");
    list_toml_files(&root)?
        .iter()
        .map(|p| {
            let text = fs::read_to_string(p)
                .map_err(|e| TableError::file(p, format!("cannot read file: {e}")))?;
            RawTable::parse(p, &text)
        })
        .collect()
}
