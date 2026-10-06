//! Typed, validated bindings for every chart and table in `data/tables/`.
//!
//! [`Tables::load`] reads the table files at runtime (so a rules profile can swap data without
//! recompiling), checks them structurally, and binds each one to a Rust type with lookup
//! functions in the vocabulary of the rules. Every lookup carries a doc comment citing its case
//! (`/// airlog:45.5`) and uses integer arithmetic only.
//!
//! Errors always name the file and the field ([`TableError`]).

pub mod airlog;
pub mod calendar;
pub mod error;
pub mod land;
pub mod ranges;
pub mod raw;
pub mod units;
pub mod validate;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub use error::TableError;
pub use raw::{Book, DiceKind, Envelope, RawTable};

/// A table with a typed binding: the id of its file and how to build it from the parsed file.
pub trait Bound: Sized {
    /// The `[table].id` of the file this type binds.
    const ID: &'static str;
    /// Build (and structurally validate) the binding from the parsed file.
    fn from_raw(raw: &RawTable) -> Result<Self, TableError>;
}

/// All parsed table files, by id.
pub struct RawSet {
    by_id: BTreeMap<String, RawTable>,
}

impl RawSet {
    /// Build from parsed files, running the envelope checks.
    pub fn new(tables: Vec<RawTable>) -> Result<Self, TableError> {
        validate::check_envelopes(&tables)?;
        Ok(Self {
            by_id: tables
                .into_iter()
                .map(|t| (t.envelope.id.clone(), t))
                .collect(),
        })
    }

    /// Bind one table; its file must exist.
    pub fn bind<T: Bound>(&self) -> Result<T, TableError> {
        let raw = self.by_id.get(T::ID).ok_or_else(|| {
            TableError::file(
                Path::new("data/tables"),
                format!("no file declares table id `{}`", T::ID),
            )
        })?;
        T::from_raw(raw)
    }

    pub fn ids(&self) -> BTreeSet<String> {
        self.by_id.keys().cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<&RawTable> {
        self.by_id.get(id)
    }
}

/// Every chart and table, loaded, validated and typed.
#[derive(Debug, Clone)]
pub struct Tables {
    pub airlog: airlog::AirlogTables,
    pub land: land::LandTables,
}

impl Tables {
    /// Load every `<data_dir>/tables/**/*.toml`. Fails on the first malformed file, naming the
    /// file and field; also checks that every table id referenced from `<data_dir>/rules`
    /// exists.
    pub fn load(data_dir: &Path) -> Result<Self, TableError> {
        let set = RawSet::new(raw::read_all(data_dir)?)?;
        validate::check_rule_references(data_dir, &set.ids())?;
        Self::bind_all(&set)
    }

    /// Bind every table from an already-parsed set.
    pub fn bind_all(set: &RawSet) -> Result<Self, TableError> {
        Ok(Self {
            airlog: airlog::AirlogTables::bind(set)?,
            land: land::LandTables::bind(set)?,
        })
    }

    /// Ids of every table that has a typed binding.
    pub fn bound_ids() -> Vec<&'static str> {
        airlog::AirlogTables::BOUND_IDS
            .iter()
            .chain(land::LandTables::BOUND_IDS)
            .copied()
            .collect()
    }
}

/// Declare a group of bound tables: a struct with one field per table, `bind` to fill it from a
/// [`RawSet`], and `BOUND_IDS` listing the ids it covers.
#[macro_export]
macro_rules! tables_group {
    ($(#[$meta:meta])* $name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $name {
            $(pub $field: $ty,)*
        }

        impl $name {
            /// Table ids this group binds.
            pub const BOUND_IDS: &'static [&'static str] = &[$(<$ty as $crate::Bound>::ID),*];

            /// Bind every table of the group from the parsed files.
            pub fn bind(set: &$crate::RawSet) -> Result<Self, $crate::TableError> {
                Ok(Self { $($field: set.bind::<$ty>()?,)* })
            }
        }
    };
}
