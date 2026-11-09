//! The facts a release PR (or any other release-notes surface) renders: what bumps, what changed and what needs a
//! reviewer's attention. Surfaces turn this into text; nothing here knows about Markdown layout.

use std::collections::HashMap;

use callisto_model::{BumpReason, ManifestFormat, PackageId, Severity};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::changed::{changed_source_files_of_uncovered, uncovered_changes, DEFAULT_IGNORED_PATHS};
use crate::changelog::markdown::{extract_markers, MarkerKind};
use crate::changelog::ChangeSource;
use crate::commands::LocalReleasePlan;
use crate::error::GraphError;
use crate::plan::{PlannedBump, VersionPlan, VersionWriteTarget};

pub const RELEASE_PREVIEW_SCHEMA_VERSION: u32 = 1;
pub const FINDING_MAJOR_BUMP: &str = "major-bump";
pub const FINDING_RELEASE_BLOCKED: &str = "release-blocked";
pub const FINDING_BREAKING_MARKER: &str = "breaking-marker";
pub const FINDING_ACTION_REQUIRED: &str = "action-required";
pub const FINDING_UNRELEASED_CHANGES: &str = "changed-not-releasing";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Where merging will publish each package. Filled in after versioning, from the versioned tree, so a preview
    /// written by `version --emit-preview` has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishOutlook>,
    /// Packages whose source changed since their last tag and that this release leaves out. Filled by `version`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreleased: Vec<UnreleasedChange>,
}

/// What planning the release of the versioned tree says, without publishing anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PublishOutlook {
    Planned {
        releases: Vec<PublishTargets>,
        /// Conditions that do not stop the release but change what it does, such as unbound tags.
        notes: Vec<String>,
    },
    /// The same planning step the release workflow runs refused; merging would fail the same way.
    Blocked { code: Option<String>, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishTargets {
    pub package: String,
    pub version: String,
    /// User-facing registry names, in publish order.
    pub registries: Vec<String>,
    pub tag: bool,
    pub github_release: bool,
    pub artifacts: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRelease {
    pub name: String,
    /// The qualified release identity (`cargo/name`), which `release --package` takes. `None` when the ecosystem
    /// could not be determined.
    #[serde(default)]
    pub release_id: Option<String>,
    pub ecosystem: String,
    pub from: String,
    pub to: String,
    pub severity: Severity,
    pub reason: Option<BumpReason>,
}

/// One underlying change, merged across every package it bumps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub kind: ChangeKind,
    /// `(package, this entry's own severity)`, not the package's post-cascade severity.
    pub members: Vec<ChangeMember>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeMember {
    pub package: String,
    pub severity: Severity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FindingLevel {
    /// Merging would fail the release.
    Stop,
    /// The reviewer should read this before merging.
    Pause,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    pub level: FindingLevel,
    pub message: String,
    /// The packages the finding is about, when it is about some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
}

/// A package with source changes since its last release tag that this release does not include.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreleasedChange {
    pub package: String,
    pub files: usize,
}

impl ReleasePreview {
    /// Reads a preview written by [`ReleasePreview`]'s `Serialize`, refusing one from another schema version.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let preview: Self = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if preview.schema_version != RELEASE_PREVIEW_SCHEMA_VERSION {
            return Err(format!(
                "schemaVersion {} is not the supported {RELEASE_PREVIEW_SCHEMA_VERSION}",
                preview.schema_version
            ));
        }
        Ok(preview)
    }
}

pub fn build_release_preview(plan: &VersionPlan) -> ReleasePreview {
    let mut sorted: Vec<&PlannedBump> = plan.bumps.iter().collect();
    sorted.sort_by_key(|bump| std::cmp::Reverse(bump.severity));
    let packages = sorted
        .into_iter()
        .map(|bump| PackageRelease {
            name: bump.package.display_name().to_string(),
            release_id: release_id(bump),
            ecosystem: ecosystem_label(bump).to_string(),
            from: bump.from.render().to_string(),
            to: bump.to.render().to_string(),
            severity: bump.severity,
            reason: bump.reason.clone(),
        })
        .collect();

    let (changes, bystanders) = collect_changes(plan);
    let findings = assess_with_changes(&plan.bumps, &changes);

    ReleasePreview {
        schema_version: RELEASE_PREVIEW_SCHEMA_VERSION,
        packages,
        changes,
        bystanders,
        findings,
        publish: None,
        unreleased: Vec::new(),
    }
}

impl ReleasePreview {
    /// Adds the packages whose source changed since their last tag and that this release leaves out. They are a
    /// finding to read, not a blocker: many such changes are rightly internal.
    pub fn with_unreleased(mut self, unreleased: Vec<UnreleasedChange>) -> Self {
        if !unreleased.is_empty() {
            self.findings.push(Finding {
                id: FINDING_UNRELEASED_CHANGES.to_owned(),
                level: FindingLevel::Pause,
                message: "Source changed since the last release, but this release does not include them. Add a changeset if the changes should ship.".to_owned(),
                packages: unreleased.iter().map(|change| change.package.clone()).collect(),
            });
        }
        self.unreleased = unreleased;
        self
    }

    /// The labels Callisto manages on the release PR, each marked with whether it applies. One list of the whole
    /// catalog, so a caller that only adds and removes by the `applied` flag never needs the names.
    pub fn pr_labels(&self) -> Vec<callisto_model::PrLabel> {
        let has = |id: &str| self.findings.iter().any(|finding| finding.id == id);
        let blocked = self.findings.iter().any(|finding| finding.level == FindingLevel::Stop);
        let breaking = has(FINDING_BREAKING_MARKER) || has(FINDING_MAJOR_BUMP);
        let action = has(FINDING_ACTION_REQUIRED);
        let unreleased = has(FINDING_UNRELEASED_CHANGES);
        let highest = self.packages.iter().map(|package| package.severity).max();
        let label = |name: &str, color: &str, description: &str, applied: bool| callisto_model::PrLabel {
            name: name.to_owned(),
            color: color.to_owned(),
            description: description.to_owned(),
            applied,
        };
        vec![
            label(
                "callisto: ready",
                "0e8a16",
                "Nothing in this release needs a second look",
                !(blocked || breaking || action || unreleased),
            ),
            label("callisto: blocked", "b60205", "Merging would fail the release", blocked),
            label(
                "callisto: breaking",
                "d93f0b",
                "A major bump or a Breaking: note",
                breaking,
            ),
            label(
                "callisto: action-required",
                "d93f0b",
                "A changeset asks users to act",
                action,
            ),
            label(
                "callisto: unreleased-changes",
                "fbca04",
                "A package changed but is not in this release",
                unreleased,
            ),
            label(
                "bump: major",
                "b60205",
                "Highest bump in this release",
                highest == Some(Severity::Major),
            ),
            label(
                "bump: minor",
                "fbca04",
                "Highest bump in this release",
                highest == Some(Severity::Minor),
            ),
            label(
                "bump: patch",
                "0e8a16",
                "Highest bump in this release",
                highest == Some(Severity::Patch),
            ),
        ]
    }

    /// The packages this release bumps, as `release --package` names them, or `None` if any could not be named. These
    /// are the packages the committed decision will hold, so planning with exactly them cannot pick up an unrelated
    /// unreleased package.
    pub fn release_selections(&self) -> Option<Vec<String>> {
        self.packages.iter().map(|package| package.release_id.clone()).collect()
    }

    /// Adds what planning the versioned tree's release said. A plan that refused is a finding that stops the release.
    pub fn with_publish(mut self, outlook: PublishOutlook) -> Self {
        if let PublishOutlook::Blocked { code, message } = &outlook {
            let message = match code {
                Some(code) => format!("{code}: {message}"),
                None => message.clone(),
            };
            self.findings.push(Finding {
                id: FINDING_RELEASE_BLOCKED.to_owned(),
                level: FindingLevel::Stop,
                message,
                packages: Vec::new(),
            });
        }
        self.publish = Some(outlook);
        self
    }
}

/// The publish outlook of `preview` on the versioned tree: plans exactly the packages the preview bumps that publish
/// somewhere. A bumped package with no publish target, such as a private member of a fixed group, is released with its
/// group but cannot be selected on its own (E169), so it is left out of the selection rather than reported as a failure.
/// `None` when a bumped package cannot be named, so nothing is claimed.
pub fn publish_outlook_in<R: callisto_model::CommandRunner, D: crate::resolver::DependencyResolver>(
    ws: &crate::Workspace<'_, R, D>,
    preview: &ReleasePreview,
) -> Option<PublishOutlook> {
    let selections: Vec<callisto_model::ReleasePackageId> = preview
        .release_selections()?
        .iter()
        .map(|id| id.parse().ok())
        .collect::<Option<_>>()?;
    let publishable: Vec<callisto_model::ReleasePackageId> = selections
        .into_iter()
        .filter(|id| {
            crate::commands::release_decision::workspace_package(ws, id)
                .is_some_and(crate::commands::release_decision::has_dispatchable_target)
        })
        .collect();
    if publishable.is_empty() {
        return Some(PublishOutlook::Planned {
            releases: Vec::new(),
            notes: Vec::new(),
        });
    }
    Some(publish_outlook(crate::commands::plan_workspace_release_offline(
        ws,
        &publishable,
    )))
}

/// Reads the outcome of planning a release into what the PR body shows. Nothing is looked up on a registry or
/// published: a refusal here is one the release itself would hit.
pub fn publish_outlook(planned: Result<Option<LocalReleasePlan>, GraphError>) -> PublishOutlook {
    let plan = match planned {
        Ok(Some(plan)) => plan,
        Ok(None) => {
            return PublishOutlook::Planned {
                releases: Vec::new(),
                notes: Vec::new(),
            }
        }
        Err(error) => {
            let code = miette::Diagnostic::code(&error).map(|code| code.to_string());
            return PublishOutlook::Blocked {
                code,
                message: error.to_string(),
            };
        }
    };

    let releases = group_operations(plan.intent.operations.iter().map(|operation| operation.id()));

    let mut notes: Vec<String> = Vec::new();
    if plan.tags_unbound {
        notes.push("`origin` has no push URL, so tags cannot be pushed until one is set.".to_owned());
    }
    notes.extend(plan.diagnostics.iter().map(|diagnostic| diagnostic.message.clone()));
    PublishOutlook::Planned { releases, notes }
}

fn release_id(bump: &PlannedBump) -> Option<String> {
    let ecosystem = callisto_model::Ecosystem::from_prefix(ecosystem_label(bump))?;
    callisto_model::ReleasePackageId::new(ecosystem, bump.package.name())
        .ok()
        .map(|id| id.to_string())
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

/// The verdict from the bumps, plus a finding for each `Breaking:` and `Action:` marker in a change's summary. A
/// breaking marker means the release cannot be called backward compatible, whatever the bump says.
fn assess_with_changes(bumps: &[PlannedBump], changes: &[Change]) -> Vec<Finding> {
    let mut findings = assess(bumps);
    let markers = marker_findings(changes);
    if markers.iter().any(|finding| finding.id == FINDING_BREAKING_MARKER) {
        findings.retain(|finding| finding.id != "compatible");
    }
    findings.extend(markers);
    findings
}

fn marker_findings(changes: &[Change]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for change in changes {
        let ChangeKind::Changeset { summary, .. } = &change.kind else {
            continue;
        };
        for marker in extract_markers(summary) {
            let id = match marker.kind {
                MarkerKind::Breaking => FINDING_BREAKING_MARKER,
                MarkerKind::Action => FINDING_ACTION_REQUIRED,
            };
            findings.push(Finding {
                id: id.to_owned(),
                level: FindingLevel::Pause,
                message: marker.text,
                packages: change.members.iter().map(|member| member.package.clone()).collect(),
            });
        }
    }
    findings
}

/// The preview of `plan` as `version` and `compose-pr-body` both show it: the plan's own facts plus the packages whose
/// source changed that the plan leaves out. One function, so the two commands cannot disagree.
pub fn build_release_preview_in<R: callisto_model::CommandRunner, D: crate::resolver::DependencyResolver>(
    ws: &crate::Workspace<'_, R, D>,
    plan: &VersionPlan,
) -> Result<ReleasePreview, GraphError> {
    Ok(build_release_preview(plan).with_unreleased(unreleased_changes(ws, plan)?))
}

/// Packages with source changes since their last tag that the plan does not bump, directly or through a group. A
/// package is only counted if it publishes somewhere, and tests, benches, examples, snapshots and Markdown do not count.
pub fn unreleased_changes<R: callisto_model::CommandRunner, D: crate::resolver::DependencyResolver>(
    ws: &crate::Workspace<'_, R, D>,
    plan: &VersionPlan,
) -> Result<Vec<UnreleasedChange>, GraphError> {
    let publishable: Vec<&callisto_model::Package> = ws
        .graph
        .packages()
        .filter(|package| crate::commands::release_decision::has_dispatchable_target(package))
        .collect();
    let bumped: std::collections::BTreeSet<PackageId> = plan.bumps.iter().map(|bump| bump.package.clone()).collect();
    let changed = changed_source_files_of_uncovered(
        &publishable,
        ws.tags()?,
        ws.git_access(),
        &bumped,
        &ws.config.groups,
        DEFAULT_IGNORED_PATHS,
    )?;
    Ok(uncovered_changes(&changed, &bumped, &ws.config.groups)
        .into_iter()
        .map(|id| UnreleasedChange {
            files: changed.get(&id).map_or(0, Vec::len),
            package: id.display_name(),
        })
        .collect())
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
            packages: Vec::new(),
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
            packages: Vec::new(),
        }];
    }
    vec![Finding {
        id: "compatible".to_owned(),
        level: FindingLevel::Info,
        message: "All changes are backward compatible.".to_owned(),
        packages: Vec::new(),
    }]
}

/// One row per released distribution. Rows are keyed by the qualified identity, so `cargo/shared` and `npm/shared`
/// stay apart; a row is labelled with the bare name unless two rows share it.
fn group_operations<'a>(
    operations: impl IntoIterator<Item = &'a callisto_model::ReleaseOperationId>,
) -> Vec<PublishTargets> {
    use callisto_model::ReleaseOperationRole;

    let mut releases: IndexMap<String, PublishTargets> = IndexMap::new();
    for id in operations {
        let qualified = id.package.to_string();
        let release = releases.entry(qualified.clone()).or_insert_with(|| PublishTargets {
            package: qualified,
            version: id.version.render().to_owned(),
            registries: Vec::new(),
            tag: false,
            github_release: false,
            artifacts: 0,
        });
        match &id.role {
            ReleaseOperationRole::RegistryPublish { registry } => {
                release
                    .registries
                    .push(registry.registry_key().display_name().to_owned());
            }
            ReleaseOperationRole::PlatformPublish { registry, .. } => {
                let name = format!("{} (platform packages)", registry.registry_key().display_name());
                if !release.registries.contains(&name) {
                    release.registries.push(name);
                }
            }
            ReleaseOperationRole::Tag => release.tag = true,
            ReleaseOperationRole::ForgeRelease => release.github_release = true,
            ReleaseOperationRole::ArtifactUpload { .. } => release.artifacts += 1,
            ReleaseOperationRole::ForgePublish => {}
        }
    }

    let mut rows: Vec<PublishTargets> = releases.into_values().collect();
    let bare = |qualified: &str| {
        qualified
            .split_once('/')
            .map_or(qualified.to_owned(), |(_, name)| name.to_owned())
    };
    for index in 0..rows.len() {
        let name = bare(&rows[index].package);
        let shared = rows.iter().filter(|row| bare(&row.package) == name).count() > 1;
        if !shared {
            rows[index].package = name;
        }
    }
    rows
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

    fn operations_for(
        distributions: &[(callisto_model::Ecosystem, &str, Version, &str)],
    ) -> Vec<callisto_model::ReleaseOperationId> {
        use callisto_model::{RegistryBindingDigest, RegistryBindingId, ReleaseOperationId, ReleasePackageId};
        let digest = RegistryBindingDigest::parse(&"0".repeat(64)).unwrap();
        let mut ids = Vec::new();
        for (ecosystem, name, version, registry) in distributions {
            let package = ReleasePackageId::new(*ecosystem, name).unwrap();
            let binding = RegistryBindingId::new(*registry, digest.clone()).unwrap();
            ids.push(ReleaseOperationId::registry_publish(
                package.clone(),
                version.clone(),
                binding,
            ));
            ids.push(ReleaseOperationId::tag(package, version.clone()));
        }
        ids
    }

    #[test]
    fn two_distributions_with_one_name_stay_two_rows_with_their_own_version_and_registry() {
        use callisto_model::Ecosystem;
        let ids = operations_for(&[
            (Ecosystem::Cargo, "shared", Version::semver(1, 0, 0), "cratesIo"),
            (Ecosystem::Npm, "shared", Version::semver(2, 0, 0), "npm"),
            (Ecosystem::Cargo, "only-cargo", Version::semver(3, 0, 0), "cratesIo"),
        ]);

        let rows = group_operations(&ids);

        let summary: Vec<(&str, &str, &[String])> = rows
            .iter()
            .map(|row| (row.package.as_str(), row.version.as_str(), row.registries.as_slice()))
            .collect();
        assert_eq!(
            summary,
            [
                ("cargo/shared", "1.0.0", &["crates.io".to_owned()][..]),
                ("npm/shared", "2.0.0", &["npm".to_owned()][..]),
                ("only-cargo", "3.0.0", &["crates.io".to_owned()][..]),
            ]
        );
    }

    #[test]
    fn a_package_that_cannot_be_named_leaves_the_release_unselected() {
        let mut plan_bump = bump(Severity::Patch);
        plan_bump.writes = vec![];
        let plan = VersionPlan {
            bumps: vec![plan_bump],
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
        // No manifest write, so no ecosystem: the preview must not claim a selection it cannot name.
        assert_eq!(build_release_preview(&plan).release_selections(), None);
    }

    fn plan_with_summary(severity: Severity, from: Version, summary: &str) -> VersionPlan {
        let mut planned = bump(severity);
        planned.from = from.clone();
        let package = planned.package.clone();
        VersionPlan {
            bumps: vec![planned],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: crate::changelog::ChangelogInput {
                    package,
                    from,
                    to: Some(Version::semver(2, 0, 0)),
                    entries: vec![crate::changelog::ChangelogEntry {
                        severity,
                        source: crate::changelog::ChangeSource::Changeset {
                            filename: "x.md".to_owned(),
                            summary: summary.to_owned(),
                        },
                    }],
                    style: Default::default(),
                },
            }],
            consumed_changesets: vec![],
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        }
    }

    fn ids(preview: &ReleasePreview) -> Vec<&str> {
        preview.findings.iter().map(|finding| finding.id.as_str()).collect()
    }

    fn applied(preview: &ReleasePreview) -> Vec<String> {
        preview
            .pr_labels()
            .into_iter()
            .filter(|label| label.applied)
            .map(|label| label.name)
            .collect()
    }

    #[test]
    fn a_clean_release_is_ready_with_its_bump_label() {
        let plan = plan_with_summary(Severity::Minor, Version::semver(1, 0, 0), "Add a flag.");
        assert_eq!(
            applied(&build_release_preview(&plan)),
            ["callisto: ready", "bump: minor"]
        );
    }

    #[test]
    fn a_breaking_marker_and_an_action_marker_label_the_release_and_it_is_not_ready() {
        let plan = plan_with_summary(
            Severity::Minor,
            Version::semver(1, 0, 0),
            "Drops a flag.\n\nBreaking: `--old` is gone.\n\nAction: switch to `--new`.",
        );
        assert_eq!(
            applied(&build_release_preview(&plan)),
            ["callisto: breaking", "callisto: action-required", "bump: minor"]
        );
    }

    #[test]
    fn a_major_bump_is_breaking_and_a_refused_plan_is_blocked() {
        let plan = plan_with_summary(Severity::Major, Version::semver(1, 0, 0), "Rewrite.");
        let preview = build_release_preview(&plan).with_publish(PublishOutlook::Blocked {
            code: Some("E320".to_owned()),
            message: "no".to_owned(),
        });
        assert_eq!(
            applied(&preview),
            ["callisto: blocked", "callisto: breaking", "bump: major"]
        );
    }

    #[test]
    fn unreleased_changes_label_the_release_and_the_whole_catalog_is_always_listed() {
        let plan = plan_with_summary(Severity::Patch, Version::semver(1, 0, 0), "Fix.");
        let preview = build_release_preview(&plan).with_unreleased(vec![UnreleasedChange {
            package: "other".to_owned(),
            files: 1,
        }]);
        assert_eq!(applied(&preview), ["callisto: unreleased-changes", "bump: patch"]);
        // Every label is present with its flag, so a caller can remove the ones that stopped applying.
        let names: Vec<String> = preview.pr_labels().into_iter().map(|label| label.name).collect();
        assert_eq!(names.len(), 8);
        assert!(names.contains(&"callisto: ready".to_owned()));
    }

    #[test]
    fn a_breaking_marker_replaces_the_compatibility_claim_and_names_the_packages() {
        let plan = plan_with_summary(
            Severity::Minor,
            Version::semver(1, 0, 0),
            "Drops a flag.\n\nBreaking: `--old` is gone.\n\nAction: switch to `--new`.",
        );

        let preview = build_release_preview(&plan);

        assert_eq!(ids(&preview), [FINDING_BREAKING_MARKER, FINDING_ACTION_REQUIRED]);
        assert_eq!(preview.findings[0].message, "`--old` is gone.");
        assert_eq!(preview.findings[0].packages, ["core-crate"]);
        assert!(preview
            .findings
            .iter()
            .all(|finding| finding.level == FindingLevel::Pause));
    }

    #[test]
    fn an_action_marker_alone_keeps_the_compatibility_claim() {
        let plan = plan_with_summary(
            Severity::Patch,
            Version::semver(1, 0, 0),
            "Fix.\n\nAction: re-run init.",
        );

        assert_eq!(
            ids(&build_release_preview(&plan)),
            ["compatible", FINDING_ACTION_REQUIRED]
        );
    }

    #[test]
    fn a_marker_in_code_or_a_list_is_not_a_finding() {
        let plan = plan_with_summary(
            Severity::Patch,
            Version::semver(1, 0, 0),
            "Fix.\n\n```md\nBreaking: example\n```\n\n- Action: example",
        );

        assert_eq!(ids(&build_release_preview(&plan)), ["compatible"]);
    }

    #[test]
    fn unreleased_changes_become_one_finding_naming_the_packages() {
        let plan = plan_with_summary(Severity::Patch, Version::semver(1, 0, 0), "Fix.");
        let preview = build_release_preview(&plan).with_unreleased(vec![
            UnreleasedChange {
                package: "a".to_owned(),
                files: 3,
            },
            UnreleasedChange {
                package: "b".to_owned(),
                files: 1,
            },
        ]);

        let finding = preview
            .findings
            .iter()
            .find(|finding| finding.id == FINDING_UNRELEASED_CHANGES)
            .expect("a finding");
        assert_eq!(finding.packages, ["a", "b"]);
        assert_eq!(finding.level, FindingLevel::Pause);

        let none = build_release_preview(&plan).with_unreleased(vec![]);
        assert!(!ids(&none).contains(&FINDING_UNRELEASED_CHANGES));
    }

    #[test]
    fn a_plan_that_refused_becomes_a_finding_that_stops_the_release() {
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
        let preview = build_release_preview(&plan).with_publish(PublishOutlook::Blocked {
            code: Some("E320".to_owned()),
            message: "no".to_owned(),
        });

        let stop: Vec<&Finding> = preview
            .findings
            .iter()
            .filter(|f| f.level == FindingLevel::Stop)
            .collect();
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0].id, FINDING_RELEASE_BLOCKED);
        assert_eq!(stop[0].message, "E320: no");
    }

    #[test]
    fn the_publish_outlook_is_left_out_of_a_preview_that_has_none_and_round_trips_when_present() {
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
        let bare = build_release_preview(&plan);
        let json = serde_json::to_string(&bare).unwrap();
        assert!(!json.contains("publish"), "{json}");
        assert_eq!(ReleasePreview::from_json(&json).unwrap(), bare);

        let with = bare.with_publish(PublishOutlook::Planned {
            releases: vec![],
            notes: vec!["n".to_owned()],
        });
        assert_eq!(
            ReleasePreview::from_json(&serde_json::to_string(&with).unwrap()).unwrap(),
            with
        );
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
