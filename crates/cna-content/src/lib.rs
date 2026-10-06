//! Loads game content from the repository's `data/` folder into typed, validated structures.
//!
//! Content is immutable for the life of a campaign and is pinned by hash (see
//! `docs/architecture.md` §3.1). This crate only reads and validates; it never invents a value
//! that the data does not contain.

pub mod map;

use std::fmt;
use std::path::PathBuf;

/// A content file could not be read or did not validate.
#[derive(Debug)]
pub enum ContentError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Csv {
        path: PathBuf,
        error: csv::Error,
    },
    Invalid {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentError::Io { path, error } => write!(f, "{}: {error}", path.display()),
            ContentError::Csv { path, error } => write!(f, "{}: {error}", path.display()),
            ContentError::Invalid { path, message } => write!(f, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for ContentError {}

/// The repository's `data/` folder, found from this crate's location. Tools and tests use it;
/// the server takes an explicit path instead.
pub fn repo_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data")
}
