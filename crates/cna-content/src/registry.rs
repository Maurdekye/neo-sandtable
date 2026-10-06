//! The rule-case registry: every numbered case of the baseline rulebooks with its disposition,
//! owning seat, place in the sequence of play and scenario applicability.
//!
//! Schema: `data/rules/README.md`. The engine uses the registry to know which cases govern each
//! step of the sequence of play, which of them it implements, and which it must refuse to skip.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::{ContentError, read_toml, toml_files};

/// All registered cases, keyed by citation (`land:8.37`, `airlog:49.13`).
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub cases: BTreeMap<String, RuleCase>,
}

/// One registered case.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuleCase {
    /// The case number within its book (`8.37`).
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    /// `rule`, `definition`, `commentary`, `example`, …
    #[serde(default)]
    pub kind: String,
    /// `automatic`, `decision`, `data`, `display`, `superseded`, `none`, `unresolved`.
    pub disposition: String,
    /// The default owning seat role for decisions, or `none`.
    #[serde(default)]
    pub seat: String,
    /// Timing anchors (`data/rules/README.md`, "timing vocabulary").
    #[serde(default)]
    pub timing: Vec<String>,
    /// Scenario applicability, e.g. `graziani = "yes" | "no" | "conditional"`.
    #[serde(default)]
    pub applies: BTreeMap<String, String>,
    #[serde(default)]
    pub tables: Vec<String>,
    #[serde(skip)]
    pub book: String,
}

impl RuleCase {
    /// `land:8.37`.
    pub fn citation(&self) -> String {
        format!("{}:{}", self.book, self.id)
    }

    /// Whether the case needs engine behaviour (an automatic procedure or a player decision).
    pub fn is_procedural(&self) -> bool {
        matches!(self.disposition.as_str(), "automatic" | "decision")
    }

    /// Whether the case applies in `scenario` (`yes` or `conditional`; unknown counts as yes).
    pub fn applies_to(&self, scenario: &str) -> bool {
        self.applies.get(scenario).is_none_or(|a| a != "no")
    }
}

#[derive(Deserialize)]
struct RegistryFile {
    #[serde(default)]
    case: Vec<RuleCase>,
}

impl Registry {
    /// Load `data/rules/<book>/*.toml` for every book folder present.
    pub fn load(rules_dir: &Path) -> Result<Self, ContentError> {
        let mut out = Registry::default();
        for book in ["land", "airlog", "scen"] {
            for path in toml_files(&rules_dir.join(book))? {
                for mut case in read_toml::<RegistryFile>(&path)?.case {
                    case.book = book.to_owned();
                    let key = case.citation();
                    if out.cases.insert(key.clone(), case).is_some() {
                        return Err(ContentError::Invalid {
                            path: path.clone(),
                            message: format!("duplicate case {key}"),
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    /// The procedural cases anchored exactly at `anchor` that apply in `scenario`, in citation
    /// order.
    pub fn procedural_at<'a>(
        &'a self,
        anchor: &'a str,
        scenario: &'a str,
    ) -> impl Iterator<Item = &'a RuleCase> + 'a {
        self.cases.values().filter(move |c| {
            c.is_procedural() && c.applies_to(scenario) && c.timing.iter().any(|t| t == anchor)
        })
    }
}
