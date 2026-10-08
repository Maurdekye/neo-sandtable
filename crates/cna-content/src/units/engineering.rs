//! Source-backed unit engineering identity; procedural eligibility belongs to the rules layer.
//! Cases: land:4.45, land:23.11, land:23.13, land:23.14, land:23.15, land:24.61

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::Weapon;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineeringScope {
    General,
    RailroadOnly,
    RoadOnly,
    AntiMineOnly,
    None,
}

/// Procedure role is distinct from the printed OA echelon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineeringRole {
    Company,
    Battalion,
    Headquarters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineeringToeRequirement {
    pub weapon: String,
    pub min_points: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineeringEvidence {
    pub transcribed_from: Vec<String>,
    pub verification: String,
}

/// Omission remains Unknown. Explicit None requires the same source verification as a positive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineeringMetadata {
    pub scope: EngineeringScope,
    pub role: Option<EngineeringRole>,
    pub toe_requirement: Option<EngineeringToeRequirement>,
    pub evidence: EngineeringEvidence,
    pub src: Vec<String>,
}

impl EngineeringMetadata {
    pub(super) fn validate(&self, weapons: &BTreeMap<String, Weapon>) -> Result<(), String> {
        if self.src.is_empty() || self.src.iter().any(|s| s.trim().is_empty()) {
            return Err("engineering metadata requires source citations".into());
        }
        if self.evidence.verification != "double"
            || self.evidence.transcribed_from.len() < 2
            || self
                .evidence
                .transcribed_from
                .iter()
                .any(|s| s.trim().is_empty())
        {
            return Err("engineering metadata requires double-read source evidence".into());
        }
        match self.scope {
            EngineeringScope::None if self.role.is_some() || self.toe_requirement.is_some() => {
                return Err("verified non-engineer identity cannot have a role or TOE gate".into());
            }
            EngineeringScope::None => {}
            _ if self.role.is_none() => {
                return Err("positive engineering scope requires a sourced procedure role".into());
            }
            _ => {}
        }
        if let Some(gate) = &self.toe_requirement {
            if !weapons.contains_key(&gate.weapon) {
                return Err(format!(
                    "engineering TOE gate references unknown weapon {}",
                    gate.weapon
                ));
            }
            if self.scope != EngineeringScope::AntiMineOnly
                || gate.weapon != "cw.scorpion"
                || gate.min_points != 6
            {
                return Err("engineering TOE gate must use the sourced land:23.15 Scorpion six-point threshold".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::{Arrival, InfantryKind, UnitRow, UnitsContent};

    /// Cases: land:4.45, land:23.11, land:23.13, land:23.14, land:24.61
    #[test]
    fn three_verified_identities_load_without_class_or_printed_echelon_inference() {
        let units = UnitsContent::load(&crate::repo_data_dir().join("units")).unwrap();
        let positives: Vec<_> = units
            .units
            .values()
            .filter(|u| u.engineering.is_some())
            .collect();
        assert_eq!(positives.len(), 3);
        for (id, scope, role) in [
            (
                "it.benghazi_garrison.viii_ii_engineer_bn",
                EngineeringScope::General,
                EngineeringRole::Battalion,
            ),
            (
                "cw.unassigned_nz.10th_nz_rr_construction_coy",
                EngineeringScope::RailroadOnly,
                EngineeringRole::Company,
            ),
            (
                "cw.unassigned_nz.13th_nz_rr_construction_coy",
                EngineeringScope::RailroadOnly,
                EngineeringRole::Company,
            ),
        ] {
            let metadata = units.units[&id.into()].engineering.as_ref().unwrap();
            assert_eq!((metadata.scope, metadata.role), (scope, Some(role)));
            assert_eq!(metadata.toe_requirement, None);
            assert_eq!(metadata.evidence.verification, "double");
        }
        let engineer = &units.units[&"it.benghazi_garrison.viii_ii_engineer_bn".into()];
        let mg = &units.units[&"it.1ccnn_div.201st_machinegun_bn".into()];
        assert_eq!(engineer.class, mg.class);
        assert_eq!(engineer.infantry_kind, None);
        assert_eq!(mg.infantry_kind, Some(InfantryKind::MachineGun));
        assert_eq!(mg.engineering, None);
        let hq = &units.units[&"cw.7_armd_div.7th_armored_div_hq".into()];
        assert!(hq.engineer_hq);
        assert_eq!(hq.engineering, None);
        assert_eq!(hq.echelon.as_deref(), Some("division"));
        for (id, gt, opstage) in [
            ("cw.unassigned_nz.10th_nz_rr_construction_coy", 32, 1),
            ("cw.unassigned_nz.13th_nz_rr_construction_coy", 50, 3),
        ] {
            let unit = &units.units[&id.into()];
            assert_eq!(unit.echelon.as_deref(), Some("battalion"));
            assert_eq!(unit.arrives, Arrival::At { gt, opstage });
        }
    }

    /// Cases: land:4.45, land:23.11, land:23.15
    #[test]
    fn missing_metadata_is_unknown_and_unknown_or_extra_tags_are_rejected() {
        let row = "id='test.unit'\nname='test'\ncounter='test'\narrives='D'\n";
        assert!(
            toml::from_str::<UnitRow>(row)
                .unwrap()
                .engineering
                .is_none()
        );
        for metadata in [
            "engineering={scope='guessed',role='company',src=['land:23.11'],evidence={verification='double',transcribed_from=['oa','counter']}}",
            "engineering={scope='general',role='brigade',src=['land:23.11'],evidence={verification='double',transcribed_from=['oa','counter']}}",
            "engineering={scope='general',role='company',protection=true,src=['land:23.11'],evidence={verification='double',transcribed_from=['oa','counter']}}",
            "engineering={scope='general',role='company',src=['land:23.11'],evidence={verification='double',transcribed_from=['oa','counter'],guessed=true}}",
        ] {
            assert!(toml::from_str::<UnitRow>(&format!("{row}{metadata}")).is_err());
        }
    }

    /// Cases: land:4.45, land:23.11, land:23.15
    #[test]
    fn loader_rejects_unverified_sources_missing_roles_and_conflicting_none() {
        let root = crate::repo_data_dir().join("units");
        let mut units = UnitsContent::load(&root).unwrap();
        let id = "it.benghazi_garrison.viii_ii_engineer_bn".into();
        let valid = units.units[&id].engineering.clone().unwrap();
        let mut missing_role = valid.clone();
        missing_role.role = None;
        let mut missing_citation = valid.clone();
        missing_citation.src.clear();
        let mut blank_citation = valid.clone();
        blank_citation.src = vec![" ".into()];
        let mut single_read = valid.clone();
        single_read.evidence.verification = "single".into();
        let mut no_counter = valid.clone();
        no_counter.evidence.transcribed_from.truncate(1);
        let mut conflicting_none = valid.clone();
        conflicting_none.scope = EngineeringScope::None;
        for bad in [
            missing_role,
            missing_citation,
            blank_citation,
            single_read,
            no_counter,
            conflicting_none,
        ] {
            units.units.get_mut(&id).unwrap().engineering = Some(bad);
            assert!(units.check(&root).is_err());
        }
        let mut verified_none = valid;
        verified_none.scope = EngineeringScope::None;
        verified_none.role = None;
        units.units.get_mut(&id).unwrap().engineering = Some(verified_none);
        units.check(&root).unwrap();
    }

    /// Cases: land:23.15
    #[test]
    fn optional_scorpion_gate_is_typed_source_bounded_and_not_a_refit_schedule() {
        let units = UnitsContent::load(&crate::repo_data_dir().join("units")).unwrap();
        // A synthetic metadata shape tests validation, not a real unit's Scorpion entitlement.
        let mut metadata = units.units[&"it.benghazi_garrison.viii_ii_engineer_bn".into()]
            .engineering
            .clone()
            .unwrap();
        metadata.scope = EngineeringScope::AntiMineOnly;
        metadata.src = vec!["land:23.15".into()];
        metadata.toe_requirement = Some(EngineeringToeRequirement {
            weapon: "cw.scorpion".into(),
            min_points: 6,
        });
        metadata.validate(&units.weapons).unwrap();
        for (scope, weapon, min_points) in [
            (EngineeringScope::General, "cw.scorpion", 6),
            (EngineeringScope::AntiMineOnly, "cw.scorpion", 0),
            (EngineeringScope::AntiMineOnly, "cw.scorpion", 5),
            (EngineeringScope::AntiMineOnly, "cw.mk_vi_light", 6),
            (EngineeringScope::AntiMineOnly, "unknown.weapon", 6),
            (EngineeringScope::None, "cw.scorpion", 6),
        ] {
            metadata.scope = scope;
            metadata.toe_requirement = Some(EngineeringToeRequirement {
                weapon: weapon.into(),
                min_points,
            });
            assert!(metadata.validate(&units.weapons).is_err());
        }
    }
}
