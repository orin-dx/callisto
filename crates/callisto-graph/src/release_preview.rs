//! The facts a release PR (or any other release-notes surface) renders: what bumps, what changed and what needs a
//! reviewer's attention. Surfaces turn this into text; nothing here knows about Markdown layout.

use std::collections::HashMap;

use callisto_model::{BumpReason, ManifestFormat, PackageId, Severity};
use indexmap::IndexMap;
use serde::Serialize;

use crate::changelog::ChangeSource;
use crate::plan::{PlannedBump, VersionPlan, VersionWriteTarget};

pub const RELEASE_PREVIEW_SCHEMA_VERSION: u32 = 1;
pub const FINDING_MAJOR_BUMP: &str = "major-bump";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasePreview {
    pub schema_version: u32,
    /// Highest severity first, ties in plan order.
    pub packages: Vec<PackageRelease>,
    /// Highest severity first, ties in first-seen order.
    pub changes: Vec<Change>,
    /// Packages released only because their group moved, with no entry of their own.
    pub bystanders: Vec<String>,
    pub findings: Vec<Finding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRelease {
    pub name: String,
    pub ecosystem: String,
    pub from: String,
    pub to: String,
    pub severity: Severity,
    pub reason: Option<BumpReason>,
}

/// One underlying change, merged across every package it bumps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub kind: ChangeKind,
    /// `(package, this entry's own severity)`, not the package's post-cascade severity.
    pub members: Vec<ChangeMember>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeMember {
    pub package: String,
    pub severity: Severity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChangeKind {
    Changeset {
        filename: String,
        summary: String,
    },
    Commit {
        subject: String,
        sha: String,
        short_sha: String,
    },
    PeerEscalation {
        dependency: String,
        to: String,
    },
    DependencyUpdate {
        dependency: String,
        to: String,
    },
    /// A package with no renderable changelog entries, described by why it bumps. Never merged across packages.
    Fallback {
        reason: Option<BumpReason>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FindingLevel {
    /// The reviewer should read this before merging.
    Pause,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    pub level: FindingLevel,
    pub message: String,
}

pub fn build_release_preview(plan: &VersionPlan) -> ReleasePreview {
    let mut sorted: Vec<&PlannedBump> = plan.bumps.iter().collect();
    sorted.sort_by_key(|bump| std::cmp::Reverse(bump.severity));
    let packages = sorted
        .into_iter()
        .map(|bump| PackageRelease {
            name: bump.package.display_name().to_string(),
            ecosystem: ecosystem_label(bump).to_string(),
            from: bump.from.render().to_string(),
            to: bump.to.render().to_string(),
            severity: bump.severity,
            reason: bump.reason.clone(),
        })
        .collect();

    let (changes, bystanders) = collect_changes(plan);

    ReleasePreview {
        schema_version: RELEASE_PREVIEW_SCHEMA_VERSION,
        packages,
        changes,
        bystanders,
        findings: assess(&plan.bumps),
    }
}

/// A bare package name carries no ecosystem prefix, so fall back to the manifest it is written to.
fn ecosystem_label(bump: &PlannedBump) -> &'static str {
    if let Some(ecosystem) = bump.package.ecosystem() {
        return ecosystem.prefix();
    }
    bump.writes
        .iter()
        .find_map(|target| match target {
            VersionWriteTarget::CargoWorkspacePackage { .. } => Some("cargo"),
            VersionWriteTarget::Manifest(path) => ManifestFormat::from_path(path)
                .ok()
                .map(|format| format.ecosystem().prefix()),
        })
        .unwrap_or("unknown")
}

/// Identity used to merge one change across every package it bumps. A shared changeset's summary is copied into each
/// package it names, which is right for each package's own changelog and noise in a multi-package PR.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ChangeKey {
    Changeset(String),
    Commit(String),
    PeerEscalation(PackageId, String),
    DependencyUpdate(PackageId, String),
    /// One per package whose changelog section could not render.
    Fallback(usize),
}

fn change_key(source: &ChangeSource) -> Option<ChangeKey> {
    match source {
        ChangeSource::Changeset { filename, .. } => Some(ChangeKey::Changeset(filename.clone())),
        ChangeSource::Commit { sha, .. } => Some(ChangeKey::Commit(sha.as_str().to_string())),
        ChangeSource::PeerEscalation { dependency, to } => {
            Some(ChangeKey::PeerEscalation(dependency.clone(), to.render().to_string()))
        }
        ChangeSource::DependencyUpdate { dependency, to, .. } => {
            Some(ChangeKey::DependencyUpdate(dependency.clone(), to.render().to_string()))
        }
        // Moved because their group did; the package table's reason column already says so.
        ChangeSource::GroupUnion { .. } | ChangeSource::NewGroupMember { .. } => None,
    }
}

/// `None` for a source with nothing to say, matching `render_section`'s own empty-summary skip.
fn change_kind(source: &ChangeSource) -> Option<ChangeKind> {
    match source {
        ChangeSource::Changeset { filename, summary } => {
            if summary.trim().is_empty() {
                return None;
            }
            Some(ChangeKind::Changeset {
                filename: filename.clone(),
                summary: summary.clone(),
            })
        }
        ChangeSource::Commit { subject, sha } => Some(ChangeKind::Commit {
            subject: subject.clone(),
            sha: sha.as_str().to_string(),
            short_sha: sha.short().to_string(),
        }),
        ChangeSource::DependencyUpdate { dependency, to, .. } => Some(ChangeKind::DependencyUpdate {
            dependency: dependency.display_name().to_string(),
            to: to.render().to_string(),
        }),
        ChangeSource::PeerEscalation { dependency, to } => Some(ChangeKind::PeerEscalation {
            dependency: dependency.display_name().to_string(),
            to: to.render().to_string(),
        }),
        ChangeSource::GroupUnion { .. } | ChangeSource::NewGroupMember { .. } => None,
    }
}

fn collect_changes(plan: &VersionPlan) -> (Vec<Change>, Vec<String>) {
    let writes: HashMap<_, _> = plan.changelog_writes.iter().map(|w| (&w.input.package, w)).collect();
    let mut groups: IndexMap<ChangeKey, Change> = IndexMap::new();
    let mut bystanders = Vec::new();
    let mut fallbacks = 0usize;

    for bump in &plan.bumps {
        let write = writes
            .get(&bump.package)
            .copied()
            .filter(|w| crate::changelog::render_section(&w.input).is_ok());

        let Some(write) = write else {
            fallbacks += 1;
            groups.insert(
                ChangeKey::Fallback(fallbacks),
                Change {
                    kind: ChangeKind::Fallback {
                        reason: bump.reason.clone(),
                    },
                    members: vec![ChangeMember {
                        package: bump.package.display_name().to_string(),
                        severity: bump.severity,
                    }],
                },
            );
            continue;
        };

        let mut substantive = false;
        for entry in &write.input.entries {
            let (Some(key), Some(kind)) = (change_key(&entry.source), change_kind(&entry.source)) else {
                continue;
            };
            substantive = true;
            groups
                .entry(key)
                .or_insert_with(|| Change {
                    kind,
                    members: Vec::new(),
                })
                .members
                .push(ChangeMember {
                    package: bump.package.display_name().to_string(),
                    severity: entry.severity,
                });
        }
        if !substantive {
            bystanders.push(bump.package.display_name().to_string());
        }
    }

    let mut changes: Vec<Change> = groups.into_values().collect();
    changes.sort_by_key(|change| std::cmp::Reverse(change.max_severity()));
    (changes, bystanders)
}

impl Change {
    pub fn max_severity(&self) -> Severity {
        self.members
            .iter()
            .map(|member| member.severity)
            .max()
            .unwrap_or(Severity::None)
    }
}

/// What a reviewer should know before merging. Below 1.0.0 a minor bump is where breaking changes go, so it is
/// never called compatible.
pub fn assess(bumps: &[PlannedBump]) -> Vec<Finding> {
    if bumps.iter().any(|bump| bump.severity == Severity::Major) {
        return vec![Finding {
            id: FINDING_MAJOR_BUMP.to_owned(),
            level: FindingLevel::Pause,
            message: "This release contains major breaking changes. Review the changes below before merging."
                .to_owned(),
        }];
    }
    let pre_one_minor = bumps
        .iter()
        .any(|bump| bump.severity == Severity::Minor && bump.from.render().starts_with("0."));
    if pre_one_minor {
        return vec![Finding {
            id: "pre-1.0-minor".to_owned(),
            level: FindingLevel::Pause,
            message: "Minor bumps below 1.0.0 can include breaking changes, so read the changes below before merging."
                .to_owned(),
        }];
    }
    vec![Finding {
        id: "compatible".to_owned(),
        level: FindingLevel::Info,
        message: "All changes are backward compatible.".to_owned(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::Version;

    fn bump(severity: Severity) -> PlannedBump {
        PlannedBump {
            package: PackageId::parse("core-crate").unwrap(),
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 0, 1),
            severity,
            governed_by: None,
            reason: None,
            writes: vec![],
        }
    }

    #[test]
    fn a_bare_package_takes_its_ecosystem_from_its_manifest() {
        let with_write = |path: &str| PlannedBump {
            writes: vec![VersionWriteTarget::Manifest(path.into())],
            ..bump(Severity::Patch)
        };
        assert_eq!(ecosystem_label(&with_write("crates/a/Cargo.toml")), "cargo");
        assert_eq!(ecosystem_label(&with_write("web/package.json")), "npm");
        assert_eq!(ecosystem_label(&with_write("py/pyproject.toml")), "pypi");
        assert_eq!(ecosystem_label(&bump(Severity::Patch)), "unknown");
    }

    #[test]
    fn preview_serializes_with_a_schema_version_and_tagged_change_kinds() {
        let plan = VersionPlan {
            bumps: vec![bump(Severity::Patch)],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![],
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };
        let json = serde_json::to_value(build_release_preview(&plan)).unwrap();
        assert_eq!(json["schemaVersion"], RELEASE_PREVIEW_SCHEMA_VERSION);
        assert_eq!(json["changes"][0]["kind"]["kind"], "fallback");
        assert_eq!(json["findings"][0]["id"], "compatible");
    }
}
