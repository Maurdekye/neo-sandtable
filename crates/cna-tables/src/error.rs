//! Errors that always name the file and the field they concern.

use std::fmt;
use std::path::{Path, PathBuf};

/// A problem with one table file: which file, which field (a dotted path such as `row[3].roll`,
/// or `<file>` when the problem concerns the whole file) and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableError {
    pub file: PathBuf,
    pub field: String,
    pub message: String,
}

impl TableError {
    pub fn new(file: &Path, field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            file: file.to_path_buf(),
            field: field.into(),
            message: message.into(),
        }
    }

    /// An error about the file as a whole.
    pub fn file(file: &Path, message: impl Into<String>) -> Self {
        Self::new(file, "<file>", message)
    }
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: field `{}`: {}",
            self.file.display(),
            self.field,
            self.message
        )
    }
}

impl std::error::Error for TableError {}
