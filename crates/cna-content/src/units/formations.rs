//! Source-defined organization slots and chart periods (land:19.3).
//! These immutable capacities never depend on current surviving units.
use super::{OaSheet, OaUnit, UnitClass};
use crate::{ContentError, read_toml, toml_files};
use cna_core::ids::UnitId;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Debug, Clone, Default)]
pub struct FormationContent {
    pub kinds: BTreeMap<String, UnitKind>,
    pub formations: BTreeMap<String, Formation>,
    pub parents: BTreeMap<UnitId, ParentOrganization>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub transcribed_from: Vec<String>,
    pub verification: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitKind {
    pub id: String,
    pub nation: String,
    pub name: String,
    pub echelon: Option<String>,
    pub symbol_echelon: Option<String>,
    #[serde(default, rename = "match")]
    pub criteria: KindMatch,
    #[serde(default)]
    pub classes: Vec<String>,
    pub sp: Option<i32>,
    #[serde(default)]
    pub fill_by: Vec<FillBy>,
    #[serde(default)]
    pub any_of_kinds: Vec<String>,
    pub note: Option<String>,
    pub src: Vec<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindMatch {
    pub unit_type: Option<String>,
    pub echelon: Option<String>,
    #[serde(default)]
    pub tags_any: Vec<String>,
    #[serde(default)]
    pub tags_all: Vec<String>,
    #[serde(default)]
    pub tags_none: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillBy {
    pub kind: String,
    pub max: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Formation {
    pub id: String,
    pub nation: String,
    pub name: String,
    pub kind: Option<String>,
    pub echelon: String,
    pub sp: i32,
    #[serde(default)]
    pub periods: Vec<Period>,
    pub designation: Option<String>,
    #[serde(default)]
    pub applies_to: Vec<UnitId>,
    pub shares_row_with: Option<String>,
    pub members: Vec<Member>,
    #[serde(default)]
    pub exceptions: Vec<Exception>,
    #[serde(default)]
    pub limits: Vec<Limit>,
    pub note: Option<String>,
    pub parent_note: Option<String>,
    pub src: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub gt_from: u16,
    pub gt_to: Option<u16>,
}
impl Period {
    pub fn contains(&self, gt: u16) -> bool {
        gt >= self.gt_from && gt <= self.gt_to.unwrap_or(u16::MAX)
    }
    fn overlaps(&self, other: &Self) -> bool {
        self.gt_from <= other.gt_to.unwrap_or(u16::MAX)
            && other.gt_from <= self.gt_to.unwrap_or(u16::MAX)
    }
}
impl Formation {
    pub fn active(&self, gt: u16) -> bool {
        gt > 0 && (self.periods.is_empty() || self.periods.iter().any(|p| p.contains(gt)))
    }
    fn effective_periods(&self) -> Vec<Period> {
        if self.periods.is_empty() {
            vec![Period {
                gt_from: 1,
                gt_to: None,
            }]
        } else {
            self.periods.clone()
        }
    }
}
/// One printed slot; alternatives share the slot. A missing SP stays missing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub kind: Option<String>,
    pub formation: Option<String>,
    pub sp: Option<i32>,
    #[serde(default)]
    pub any_of: Vec<Member>,
    pub style: Option<String>,
    pub gap: Option<String>,
    pub glyph_note: Option<String>,
    pub echelon_mark: Option<String>,
    pub note: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    pub holder_sheet: String,
    pub add: Member,
    pub note: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    pub what: String,
    pub max: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentOrganization {
    pub unit: UnitId,
    /// Exactly one source of capacity; an explicitly empty OA list means zero slots.
    pub profiles: Option<Vec<String>>,
    pub oa_slots: Option<Vec<UnitId>>,
    pub evidence: Evidence,
    pub src: Vec<String>,
    pub attachment_maximum: Option<AttachmentMaximum>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentMaximum {
    pub units: i32,
    pub evidence: Evidence,
    pub src: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileMetadata {
    nation: String,
    src: Vec<String>,
    transcribed_from: Vec<String>,
    verification: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FormationFile {
    file: FileMetadata,
    #[serde(default)]
    kind: Vec<UnitKind>,
    #[serde(default)]
    formation: Vec<Formation>,
    #[serde(default)]
    parent: Vec<ParentOrganization>,
}
fn evidence_ok(e: &Evidence) -> bool {
    e.verification == "double"
        && e.transcribed_from.len() >= 2
        && e.transcribed_from.iter().all(|s| !s.is_empty())
}
fn echelon_ok(e: &str) -> bool {
    matches!(
        e,
        "company" | "battalion" | "brigade" | "super_brigade" | "division" | "battle_group"
    )
}
impl FormationContent {
    pub(super) fn load(
        dir: &Path,
        classes: &BTreeMap<String, UnitClass>,
        units: &BTreeMap<UnitId, OaUnit>,
        sheets: &BTreeMap<String, OaSheet>,
    ) -> Result<Self, ContentError> {
        let mut out = Self::default();
        for path in toml_files(dir)? {
            // read_toml records every consumed chart file through note_read.
            let file: FormationFile = read_toml(&path)?;
            out.add_file(file, &path)?;
        }
        out.check(dir, classes, units, sheets)?;
        Ok(out)
    }
    fn add_file(&mut self, file: FormationFile, path: &Path) -> Result<(), ContentError> {
        let meta = &file.file;
        if meta.src.is_empty()
            || meta.transcribed_from.is_empty()
            || meta.transcribed_from.iter().any(String::is_empty)
            || meta.verification != "double"
        {
            return Err(ContentError::Invalid {
                path: path.to_path_buf(),
                message: "formation chart needs double-read source evidence".into(),
            });
        }
        for kind in file.kind {
            if kind.nation != meta.nation {
                return Err(ContentError::Invalid {
                    path: path.to_path_buf(),
                    message: "kind nation differs from chart".into(),
                });
            }
            super::insert_unique(&mut self.kinds, kind.id.clone(), kind, path)?;
        }
        for formation in file.formation {
            if formation.nation != meta.nation {
                return Err(ContentError::Invalid {
                    path: path.to_path_buf(),
                    message: "formation nation differs from chart".into(),
                });
            }
            super::insert_unique(&mut self.formations, formation.id.clone(), formation, path)?;
        }
        for parent in file.parent {
            super::insert_unique(&mut self.parents, parent.unit.clone(), parent, path)?;
        }
        Ok(())
    }
    /// None is a source gap, including a mapped parent with no profile at this date.
    /// The procedure must explicitly refuse that change; no nearest-period fallback.
    pub fn profile(&self, unit: &UnitId, gt: u16) -> Option<&Formation> {
        self.parents
            .get(unit)?
            .profiles
            .as_ref()?
            .iter()
            .filter_map(|id| self.formations.get(id))
            .find(|f| f.active(gt))
    }
    fn check(
        &self,
        path: &Path,
        classes: &BTreeMap<String, UnitClass>,
        units: &BTreeMap<UnitId, OaUnit>,
        sheets: &BTreeMap<String, OaSheet>,
    ) -> Result<(), ContentError> {
        let fail = |message: String| ContentError::Invalid {
            path: path.to_path_buf(),
            message,
        };
        for k in self.kinds.values() {
            if k.id.is_empty()
                || !k.id.starts_with(&format!("{}.", k.nation))
                || k.name.is_empty()
                || k.src.is_empty()
                || k.sp.is_some_and(|s| s < 0)
                || k.echelon.as_deref().is_some_and(|e| !echelon_ok(e))
                || k.criteria
                    .echelon
                    .as_deref()
                    .is_some_and(|e| !echelon_ok(e))
                || k.symbol_echelon
                    .as_deref()
                    .is_some_and(|e| !matches!(e, "I" | "II" | "III" | "X" | "XX"))
            {
                return Err(fail(format!("invalid formation kind {}", k.id)));
            }
            if let Some(t) = &k.criteria.unit_type
                && !classes.values().any(|c| &c.unit_type == t)
            {
                return Err(fail(format!("{}: unknown unit type {t}", k.id)));
            }
            for c in &k.classes {
                if !classes.contains_key(c) {
                    return Err(fail(format!("{}: unknown class {c}", k.id)));
                }
            }
            for other in k
                .any_of_kinds
                .iter()
                .chain(k.fill_by.iter().map(|f| &f.kind))
            {
                if !self.kinds.contains_key(other) {
                    return Err(fail(format!("{}: unknown kind {other}", k.id)));
                }
            }
            if k.fill_by.iter().any(|f| f.max <= 0) {
                return Err(fail(format!(
                    "{}: nonpositive replacement-slot capacity",
                    k.id
                )));
            }
        }
        for f in self.formations.values() {
            if !f.id.starts_with(&format!("{}.", f.nation))
                || f.name.is_empty()
                || !echelon_ok(&f.echelon)
                || f.sp < 0
                || f.members.is_empty()
                || f.src.is_empty()
                || f.limits.iter().any(|l| l.what.is_empty() || l.max < 0)
            {
                return Err(fail(format!("invalid formation {}", f.id)));
            }
            if let Some(k) = &f.kind
                && !self.kinds.contains_key(k)
            {
                return Err(fail(format!("{}: unknown kind {k}", f.id)));
            }
            if let Some(other) = &f.shares_row_with
                && !self.formations.contains_key(other)
            {
                return Err(fail(format!("{}: unknown shared row {other}", f.id)));
            }
            for u in &f.applies_to {
                if !units.contains_key(u) {
                    return Err(fail(format!("{}: unknown OA unit {u}", f.id)));
                }
            }
            for (i, p) in f.periods.iter().enumerate() {
                if p.gt_from == 0
                    || p.gt_to.is_some_and(|n| n < p.gt_from)
                    || f.periods[..i].iter().any(|other| p.overlaps(other))
                {
                    return Err(fail(format!("{}: invalid or overlapping periods", f.id)));
                }
            }
            for m in &f.members {
                self.check_member(m, true, path)?;
            }
            for e in &f.exceptions {
                if !sheets.contains_key(&e.holder_sheet) {
                    return Err(fail(format!(
                        "{}: unknown exception sheet {}",
                        f.id, e.holder_sheet
                    )));
                }
                self.check_member(&e.add, true, path)?;
            }
        }
        for p in self.parents.values() {
            let Some(oa) = units.get(&p.unit) else {
                return Err(fail(format!("unknown organization parent {}", p.unit)));
            };
            if p.profiles.is_some() == p.oa_slots.is_some()
                || !evidence_ok(&p.evidence)
                || p.src.is_empty()
            {
                return Err(fail(format!(
                    "{}: exactly one evidenced organization capacity is required",
                    p.unit
                )));
            }
            if let Some(max) = &p.attachment_maximum
                && (max.units < 0 || !evidence_ok(&max.evidence) || max.src.is_empty())
            {
                return Err(fail(format!(
                    "{}: invalid attachment maximum evidence",
                    p.unit
                )));
            }
            if let Some(slots) = &p.oa_slots {
                let mut seen = BTreeSet::new();
                for slot in slots {
                    if !seen.insert(slot)
                        || !units
                            .get(slot)
                            .is_some_and(|u| u.parent.as_ref() == Some(&p.unit))
                    {
                        return Err(fail(format!(
                            "{}: OA slot {slot} is not a unique printed child",
                            p.unit
                        )));
                    }
                }
            }
            if let Some(profiles) = &p.profiles {
                if profiles.is_empty() {
                    return Err(fail(format!("{}: empty profile list", p.unit)));
                }
                let mut dates = Vec::new();
                for id in profiles {
                    let Some(f) = self.formations.get(id) else {
                        return Err(fail(format!("{}: unknown formation {id}", p.unit)));
                    };
                    if sheets[&oa.sheet].nation != f.nation
                        || !f.applies_to.is_empty() && !f.applies_to.contains(&p.unit)
                    {
                        return Err(fail(format!("{}: incompatible formation {id}", p.unit)));
                    }
                    for period in f.effective_periods() {
                        if dates.iter().any(|d| period.overlaps(d)) {
                            return Err(fail(format!(
                                "{}: ambiguous inclusive profile periods",
                                p.unit
                            )));
                        }
                        dates.push(period);
                    }
                }
            }
        }
        // Composition and kind substitution graphs must terminate, independently of OA state.
        for id in self.formations.keys() {
            self.check_cycles(id, false, &mut BTreeSet::new(), path)?;
        }
        for id in self.kinds.keys() {
            self.check_cycles(id, true, &mut BTreeSet::new(), path)?;
        }
        Ok(())
    }
    fn check_member(&self, m: &Member, top: bool, path: &Path) -> Result<(), ContentError> {
        let fail = |s: &str| ContentError::Invalid {
            path: path.to_path_buf(),
            message: s.into(),
        };
        if !m.any_of.is_empty() {
            if !top
                || m.any_of.len() < 2
                || m.kind.is_some()
                || m.formation.is_some()
                || m.sp.is_some()
                || m.style.is_some()
                || m.gap.is_some()
                || m.glyph_note.is_some()
                || m.echelon_mark.is_some()
            {
                return Err(fail("ambiguous formation alternative slot"));
            }
            for choice in &m.any_of {
                self.check_member(choice, false, path)?;
            }
            return Ok(());
        }
        if m.kind.is_none() && m.formation.is_none() || m.sp.is_some_and(|n| n < 0) {
            return Err(fail("invalid formation slot"));
        }
        if let Some(k) = &m.kind {
            let Some(kind) = self.kinds.get(k) else {
                return Err(fail("slot names unknown kind"));
            };
            if let (Some(a), Some(b)) = (m.sp, kind.sp)
                && a != b
            {
                return Err(fail("slot stacking value differs from kind"));
            }
        }
        if let Some(f) = &m.formation {
            let Some(formation) = self.formations.get(f) else {
                return Err(fail("slot names unknown formation"));
            };
            if m.sp.is_some_and(|n| n != formation.sp)
                || m.kind.is_some() && formation.kind.is_some() && m.kind != formation.kind
            {
                return Err(fail("slot differs from referenced formation"));
            }
        }
        Ok(())
    }
    fn check_cycles(
        &self,
        id: &str,
        kind: bool,
        seen: &mut BTreeSet<String>,
        path: &Path,
    ) -> Result<(), ContentError> {
        if !seen.insert(id.into()) {
            return Err(ContentError::Invalid {
                path: path.to_path_buf(),
                message: format!("cyclic organization reference {id}"),
            });
        }
        if kind {
            let k = &self.kinds[id];
            for child in k
                .any_of_kinds
                .iter()
                .chain(k.fill_by.iter().map(|f| &f.kind))
            {
                self.check_cycles(child, true, seen, path)?;
            }
        } else {
            let f = &self.formations[id];
            for m in f.members.iter().chain(f.exceptions.iter().map(|e| &e.add)) {
                for slot in std::iter::once(m).chain(m.any_of.iter()) {
                    if let Some(child) = &slot.formation {
                        self.check_cycles(child, false, seen, path)?;
                    }
                }
            }
        }
        seen.remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{repo_data_dir, units::UnitsContent};
    fn units() -> UnitsContent {
        UnitsContent::load(&repo_data_dir().join("units")).unwrap()
    }
    fn parent() -> ParentOrganization {
        ParentOrganization {
            unit: "cw.7_armd_div.7th_armored_div_hq".into(),
            profiles: Some(
                [
                    "cw.armd_div_i",
                    "cw.armd_div_ii",
                    "cw.armd_div_iii",
                    "cw.armd_div_iv",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ),
            oa_slots: None,
            evidence: Evidence {
                transcribed_from: vec!["fixture:OA".into(), "fixture:chart".into()],
                verification: "double".into(),
            },
            src: vec!["land:19.31".into()],
            attachment_maximum: None,
        }
    }
    /// Cases: land:19.31, land:19.32, land:19.33
    #[test]
    fn actual_charts_are_typed_and_all_reads_are_recorded() {
        let (u, reads) = crate::record_reads(units);
        assert_eq!(u.formations.kinds.len(), 47);
        assert_eq!(u.formations.formations.len(), 38);
        for file in ["cw.toml", "it.toml", "ge.toml"] {
            assert!(reads.contains(&crate::normalize(
                &repo_data_dir().join("units/formations").join(file)
            )));
        }
        let f = &u.formations.formations["cw.armd_div_iv"];
        assert_eq!(f.members.len(), 10);
        assert_eq!(f.members.last().unwrap().any_of.len(), 2);
        assert_eq!(u.formations.kinds["it.at_bn"].fill_by[0].max, 3);
    }
    /// Cases: land:4.45, land:19.3, land:19.31
    #[test]
    fn actual_7th_armored_parents_select_source_profiles_and_track_reads() {
        let (u, reads) = crate::record_reads(units);
        for file in ["units/formations/cw.toml", "units/oa/cw/7_armd_div.toml"] {
            assert!(reads.contains(&crate::normalize(&repo_data_dir().join(file))));
        }
        for (unit, prefix, ids) in [
            (
                "cw.7_armd_div.7th_armored_div_hq",
                "cw.armd_div_",
                vec![
                    "cw.armd_div_i",
                    "cw.armd_div_ii",
                    "cw.armd_div_iii",
                    "cw.armd_div_iv",
                ],
            ),
            (
                "cw.7_armd_div.4th_armored_bde_hq",
                "cw.armd_bde_",
                vec!["cw.armd_bde_i", "cw.armd_bde_ii", "cw.armd_bde_iii"],
            ),
            (
                "cw.7_armd_div.7th_armored_bde_hq",
                "cw.armd_bde_",
                vec!["cw.armd_bde_i", "cw.armd_bde_ii", "cw.armd_bde_iii"],
            ),
        ] {
            let id = UnitId::new(unit);
            let mapping = &u.formations.parents[&id];
            assert_eq!(mapping.profiles.as_ref().unwrap(), &ids);
            assert!(mapping.oa_slots.is_none());
            assert!(mapping.attachment_maximum.is_none());
            assert_eq!(mapping.evidence.verification, "double");
            assert_eq!(
                mapping.evidence.transcribed_from,
                [
                    "vassal:OC BR 7th Armoured Division-01.png",
                    "vassal:Allied Formation Chart.png",
                ]
            );
            for gt in [1, 18, 19, 70, 71, 91, 92, 111] {
                let suffix = match gt {
                    1 | 18 => "i",
                    19 | 70 => "ii",
                    71 | 91 => "iii",
                    _ if prefix == "cw.armd_div_" => "iv",
                    _ => "ii",
                };
                let profile = u.formations.profile(&id, gt).unwrap();
                assert_eq!(profile.id, format!("{prefix}{suffix}"));
                assert_eq!(profile.echelon, u.units[&id].echelon.as_deref().unwrap());
                assert_eq!(
                    ids.iter()
                        .filter(|name| u.formations.formations[**name].active(gt))
                        .count(),
                    1
                );
            }
            assert!(u.formations.profile(&id, 0).is_none());
            let mut incomplete = u.formations.clone();
            incomplete
                .parents
                .get_mut(&id)
                .unwrap()
                .profiles
                .as_mut()
                .unwrap()
                .remove(1);
            for gt in [19, 70] {
                assert!(incomplete.profile(&id, gt).is_none());
            }
            if prefix == "cw.armd_bde_" {
                assert!(incomplete.profile(&id, 92).is_none());
            }
        }
        // A standard brigade's chart maximum remains three tank battalions,
        // despite the two printed battalions under each of these OA HQs.
        let slots = &u.formations.formations["cw.armd_bde_i"].members;
        assert_eq!(slots.len(), 3);
        assert!(
            slots
                .iter()
                .all(|slot| slot.kind.as_deref() == Some("cw.tank_bn"))
        );
        for parent in [
            "cw.7_armd_div.4th_armored_bde_hq",
            "cw.7_armd_div.7th_armored_bde_hq",
        ] {
            let id = UnitId::new(parent);
            assert_eq!(
                u.units
                    .values()
                    .filter(|unit| unit.parent.as_ref() == Some(&id))
                    .count(),
                2
            );
        }
    }
    /// Cases: land:19.25, land:19.31
    #[test]
    fn inclusive_dates_choose_exact_profile_and_never_bridge_gaps() {
        let u = units();
        let mut forms = u.formations.clone();
        let p = parent();
        let id = p.unit.clone();
        forms.parents.insert(id.clone(), p);
        forms
            .check(Path::new("fixture"), &u.classes, &u.units, &u.sheets)
            .unwrap();
        for (gt, profile) in [
            (1, "cw.armd_div_i"),
            (18, "cw.armd_div_i"),
            (19, "cw.armd_div_ii"),
            (70, "cw.armd_div_ii"),
            (71, "cw.armd_div_iii"),
            (91, "cw.armd_div_iii"),
            (92, "cw.armd_div_iv"),
        ] {
            assert_eq!(forms.profile(&id, gt).unwrap().id, profile);
        }
        forms
            .parents
            .get_mut(&id)
            .unwrap()
            .profiles
            .as_mut()
            .unwrap()
            .remove(1);
        assert!(forms.profile(&id, 19).is_none());
        assert!(forms.profile(&id, 70).is_none());
    }
    /// Cases: land:19.3, land:4.45
    #[test]
    fn duplicate_parent_rows_are_rejected_at_the_actual_file_binding() {
        let path = repo_data_dir().join("units/formations/cw.toml");
        let mut file: FormationFile = read_toml(&path).unwrap();
        file.parent = vec![parent(), parent()];
        assert!(FormationContent::default().add_file(file, &path).is_err());
    }
    /// Cases: land:19.3, land:4.45
    #[test]
    fn ambiguous_missing_and_unverified_parent_mappings_are_rejected() {
        let u = units();
        let mut forms = u.formations.clone();
        let p = parent();
        let id = p.unit.clone();
        forms.parents.insert(id.clone(), p.clone());
        let rejected = |f: &FormationContent| {
            assert!(
                f.check(Path::new("fixture"), &u.classes, &u.units, &u.sheets)
                    .is_err()
            )
        };
        let mut bad = forms.clone();
        bad.parents
            .get_mut(&id)
            .unwrap()
            .profiles
            .as_mut()
            .unwrap()
            .push("missing".into());
        rejected(&bad);
        let mut bad = forms.clone();
        bad.formations.get_mut("cw.armd_div_ii").unwrap().periods[0].gt_from = 18;
        rejected(&bad);
        let mut bad = forms.clone();
        bad.parents.get_mut(&id).unwrap().oa_slots = Some(vec![]);
        rejected(&bad);
        let mut bad = forms.clone();
        bad.parents.get_mut(&id).unwrap().profiles = None;
        rejected(&bad);
        let mut bad = forms.clone();
        bad.parents.get_mut(&id).unwrap().evidence.verification = "single".into();
        rejected(&bad);
        let mut bad = forms.clone();
        let b = bad.parents.get_mut(&id).unwrap();
        b.profiles = None;
        b.oa_slots = Some(vec!["missing".into()]);
        rejected(&bad);
        let mut empty = forms.clone();
        let p = empty.parents.get_mut(&id).unwrap();
        p.profiles = None;
        p.oa_slots = Some(vec![]);
        empty
            .check(Path::new("fixture"), &u.classes, &u.units, &u.sheets)
            .unwrap();
        let mut cyclic = forms;
        cyclic.formations.get_mut("cw.armd_div_i").unwrap().members[0].formation =
            Some("cw.armd_div_i".into());
        cyclic.formations.get_mut("cw.armd_div_i").unwrap().members[0].sp = Some(5);
        rejected(&cyclic);
    }
}
