use std::collections::BTreeMap;
use std::path::PathBuf;

use callisto_changelog::ChangelogInput;
use callisto_format::PreState;
use callisto_model::{
    BumpReason, BumpRecord, CommitSha, ConfigKey, Diagnostic, LockfileRefreshResult, PackageId,
    Severity, Version, VersionReport, SCHEMA_VERSION,
};

use crate::cascade::SpecRewrite;

#[derive(Clone, Debug, Default)]
pub struct VersionPlan {
    pub bumps: Vec<PlannedBump>,
    pub rewrites: Vec<SpecRewrite>,
    pub platform_writes: Vec<PlatformWrite>,
    pub optional_dep_updates: Vec<OptionalDepUpdate>,
    pub changelog_writes: Vec<ChangelogWrite>,
    pub consumed_changesets: Vec<PathBuf>,
    pub pre_state_update: Option<PreState>,
    pub delete_pre_json: Option<PathBuf>,
    pub pre_cursor_updates: Vec<(PackageId, CommitSha)>,
    pub observed_versions: BTreeMap<PackageId, Version>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedBump {
    pub package: PackageId,
    pub from: Version,
    pub to: Version,
    pub severity: Severity,
    pub governed_by: Option<ConfigKey>,
    pub reason: Option<BumpReason>,
    pub writes: Vec<VersionWriteTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum VersionWriteTarget {
    Manifest(PathBuf),
    CargoWorkspacePackage { root_manifest: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformWrite {
    pub manifest: PathBuf,
    pub version: Version,
    pub from: Version,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionalDepUpdate {
    pub manifest: PathBuf,
    pub updates: Vec<(String, Version)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogWrite {
    pub changelog_path: PathBuf,
    pub input: ChangelogInput,
}

impl VersionPlan {
    pub fn to_report(&self, lockfiles: Option<Vec<LockfileRefreshResult>>) -> VersionReport {
        let mut bumps = Vec::new();
        for b in &self.bumps {
            bumps.push(BumpRecord {
                package: b.package.clone(),
                from: b.from.clone(),
                to: b.to.clone(),
                severity: b.severity,
                governed_by: b.governed_by.clone(),
                reason: b.reason.clone(),
            });
        }

        VersionReport {
            schema_version: SCHEMA_VERSION,
            bumps,
            lockfile_refresh_results: lockfiles,
            diagnostics: self.diagnostics.clone(),
        }
    }
}

#[cfg(test)]
mod platform_write_from_field_tests {
    use super::*;
    use callisto_model::VersionGrammar;

    #[test]
    fn platform_write_carries_from_field_distinct_from_version() {
        let pw = PlatformWrite {
            manifest: PathBuf::from("platform/package.json"),
            version: Version::parse("1.1.0", VersionGrammar::SemVer).unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
        };
        assert_eq!(pw.from.render(), "1.0.0");
        assert_eq!(pw.version.render(), "1.1.0");
    }
}
