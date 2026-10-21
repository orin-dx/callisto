use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::changelog::ChangelogInput;
use callisto_model::format::PreState;
use callisto_model::{
    BumpReason, BumpRecord, CommitSha, ConfigKey, Diagnostic, LockfileRefreshResult, PackageId, Severity, Version,
    VersionReport, PACKAGE_REPORT_SCHEMA_VERSION,
};

/// Each workspace package's id as reports print it: `ecosystem/name` for its primary ecosystem.
#[derive(Clone, Debug, Default)]
pub struct ReportIds(BTreeMap<PackageId, PackageId>);

impl ReportIds {
    pub fn new<'a>(packages: impl Iterator<Item = &'a callisto_model::Package>) -> Self {
        Self(
            packages
                .map(|package| (package.id.clone(), package.canonical_id()))
                .collect(),
        )
    }

    /// The report id of `id`; an id naming no workspace package is returned as it is.
    pub fn of(&self, id: &PackageId) -> PackageId {
        self.0.get(id).cloned().unwrap_or_else(|| id.clone())
    }

    /// Rewrites every package a diagnostic names to its report id.
    pub fn diagnostics(&self, diagnostics: &mut [Diagnostic]) {
        for diagnostic in diagnostics {
            if let Some(package) = &diagnostic.package {
                diagnostic.package = Some(self.of(package));
            }
        }
    }

    /// Rewrites the package a bump reason points at to its report id.
    pub fn reason(&self, reason: &mut BumpReason) {
        if let BumpReason::Cascade { via, .. } | BumpReason::PeerEscalation { via, .. } = reason {
            *via = self.of(via);
        }
    }
}

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
    /// Workspace-relative `pre.json` path, from `ResolvedConfig::pre_json_path`. The single source
    /// `apply_version_plan` reads instead of guessing the changesets dir from `consumed_changesets`, which is
    /// empty in pre mode.
    pub pre_json_path: PathBuf,
    /// Raw `pre.json` text as read from disk this run, so a write of `pre_state_update` can preserve its line
    /// endings, BOM and indent instead of the default 2-space bare-LF shape. `None` when no `pre.json` existed.
    pub pre_json_original_text: Option<String>,
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
    pub fn to_report(&self, ids: &ReportIds, lockfiles: Option<Vec<LockfileRefreshResult>>) -> VersionReport {
        let mut bumps = Vec::new();
        for b in &self.bumps {
            let mut reason = b.reason.clone();
            if let Some(reason) = &mut reason {
                ids.reason(reason);
            }
            bumps.push(BumpRecord {
                package: ids.of(&b.package),
                from: b.from.clone(),
                to: b.to.clone(),
                severity: b.severity,
                governed_by: b.governed_by.clone(),
                reason,
            });
        }
        let mut diagnostics = self.diagnostics.clone();
        ids.diagnostics(&mut diagnostics);

        VersionReport {
            schema_version: PACKAGE_REPORT_SCHEMA_VERSION,
            bumps,
            lockfile_refresh_results: lockfiles,
            diagnostics,
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
