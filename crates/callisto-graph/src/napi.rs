use std::collections::BTreeMap;
use std::path::Path;

use callisto_model::{Diagnostic, DiagnosticCode, DiagnosticSeverity, ManifestRole, StrictFlag};

use crate::config::{GroupDef, GroupTable};

#[derive(Clone, Debug, Default)]
pub struct NapiTargetsIndex {
    declared: BTreeMap<callisto_model::GroupName, Vec<String>>,
}

impl NapiTargetsIndex {
    pub fn load(groups: &GroupTable, _root: &Path) -> Result<Self, callisto_model::ManifestError> {
        let mut declared = BTreeMap::new();
        for g in groups.fixed.values() {
            declared.insert(g.name.clone(), Vec::new());
        }
        Ok(NapiTargetsIndex { declared })
    }

    pub fn declared_for(&self, group: &callisto_model::GroupName) -> Option<&[String]> {
        self.declared.get(group).map(|v| v.as_slice())
    }
}

pub fn napi_drift(group: &GroupDef, declared: &[String], _root: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let declared_triples: Vec<String> = declared.iter().map(|s| s.trim().to_string()).collect();

    for t in &declared_triples {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::NapiTargetAddedNotInMembers,
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "`napi.targets` declares `{t}`, which is not in fixed group `{}`'s members; accept it with `callisto init`",
                group.name
            ),
            package: None,
            path: None,
            governed_by: None,
            escalated_by: Some(StrictFlag::Strict),
        });
    }

    diagnostics
}

pub fn triple_to_role(triple: &str) -> Option<ManifestRole> {
    let parts: Vec<&str> = triple.split('-').collect();
    if parts.len() >= 3 {
        let arch = parts[0].to_string();
        let platform = parts[2].to_string();
        Some(ManifestRole::Platform {
            platform,
            arch,
            abi: None,
        })
    } else {
        None
    }
}

pub fn role_to_triple(role: &ManifestRole) -> Option<String> {
    if let ManifestRole::Platform {
        platform,
        arch,
        abi,
    } = role
    {
        Some(format!(
            "{}-{}-{}",
            arch,
            platform,
            abi.as_deref().unwrap_or("gnu")
        ))
    } else {
        None
    }
}
