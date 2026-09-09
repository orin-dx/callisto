//! Deterministic, graph-owned release roster decisions.
//!
//! This is deliberately separate from `PublishPlan`: it records the exact
//! package/version authority that later intent construction consumes, without
//! exposing a mutation route.

use callisto_model::{
    BumpReason, CommandRunner, CommitSha, Ecosystem, Package, ReleaseDecisionEntry, ReleaseDecisionV1,
    ReleaseInclusionReason, ReleasePackageId, Version,
};

use crate::{DependencyResolver, GraphError, VersionPlan, Workspace};

/// Computes one [`ReleasePackageId`] per canonical manifest, ecosystem-qualified
/// against `package`'s name.
///
/// This is the single derivation of release package identity shared by every
/// site that computes or verifies release authority: [`derive_release_decision`],
/// [`derive_release_commit_decision`], and `release::derive_release_inputs`.
/// Per this module's own established lesson (see the doc comment on
/// [`derive_release_commit_decision`]), independently reimplementing this
/// mapping at each call site is exactly the kind of duplicated derivation
/// that drifts -- so it lives here once instead.
pub(crate) fn release_package_ids(package: &Package) -> Result<Vec<ReleasePackageId>, GraphError> {
    package
        .canonical_manifests()
        .map(|manifest| ReleasePackageId::new(manifest.ecosystem(), package.id.name()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_error| GraphError::ReleaseIntentStale)
}

/// Derives the durable roster from a freshly computed version plan.
///
/// The caller supplies the plan from the same workspace observation; this
/// function never inspects `PublishPlan` or a caller-provided release roster.
pub fn derive_release_decision<R: callisto_model::CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    plan: &VersionPlan,
) -> Result<ReleaseDecisionV1, GraphError> {
    let mut package_ids = std::collections::BTreeMap::new();
    for package in workspace.graph.packages() {
        let ids = release_package_ids(package)?;
        package_ids.insert(package.id.clone(), ids);
    }

    let mut entries = Vec::new();
    for bump in &plan.bumps {
        let ids = package_ids.get(&bump.package).ok_or(GraphError::ReleaseIntentStale)?;
        for id in ids {
            entries.push(ReleaseDecisionEntry {
                package: id.clone(),
                target_version: bump.to.clone(),
                reasons: vec![reason_from_bump(bump.reason.as_ref(), &package_ids)?],
            });
        }
    }
    ReleaseDecisionV1::new(entries).map_err(|_error| GraphError::ReleaseIntentStale)
}

/// Derives a durable decision for explicit, exact release identities.
///
/// A linked group is one release unit: selecting any member includes every
/// member of that linked group which the version plan selected. All other
/// packages remain outside the authority boundary. The caller must pass
/// ecosystem-qualified [`ReleasePackageId`] values; this function never uses
/// `PackageId::matches`, whose bare-name wildcard semantics are unsuitable
/// for release authority.
pub fn derive_selected_release_decision<R: callisto_model::CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    plan: &VersionPlan,
    selections: &[ReleasePackageId],
) -> Result<ReleaseDecisionV1, GraphError> {
    let complete = derive_release_decision(workspace, plan)?;
    let selected = selections.iter().collect::<std::collections::BTreeSet<_>>();
    if selected.len() != selections.len() {
        return Err(GraphError::ReleaseIntentStale);
    }
    if selected
        .iter()
        .any(|selection| !complete.entries.iter().any(|entry| &entry.package == *selection))
    {
        return Err(GraphError::ReleaseIntentStale);
    }

    let linked_groups = complete
        .entries
        .iter()
        .filter(|entry| selected.contains(&entry.package))
        .flat_map(|entry| entry.reasons.iter())
        .filter_map(|reason| match reason {
            ReleaseInclusionReason::LinkedGroup { group_id } => Some(group_id.clone()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();

    let entries = complete
        .entries
        .into_iter()
        .filter(|entry| {
            selected.contains(&entry.package)
                || entry.reasons.iter().any(|reason| {
                    matches!(reason, ReleaseInclusionReason::LinkedGroup { group_id } if linked_groups.contains(group_id))
                })
        })
        .collect();
    ReleaseDecisionV1::new(entries).map_err(|_error| GraphError::ReleaseIntentStale)
}

/// Verifies the release roster a merged release commit claims, against a
/// release-decision file committed alongside it at `decision_path`.
///
/// This is intentionally *not* a second computation of changeset, fixed-group,
/// linked-group, cascade, or pre-release-policy inclusion. `plan_version`
/// (via [`derive_release_decision`]) already computed that once, correctly,
/// when the release PR was generated -- and `callisto version --emit-decision`
/// commits its exact output alongside the manifest and changelog edits.
/// Re-deriving that policy a second time here, from raw git diffs, is exactly
/// the kind of duplicated logic that drifts: an earlier version of this
/// function did just that, understood only a direct changeset match, and
/// rejected every real release in this repository once a fixed-group cascade
/// (a case its reimplementation never learned) touched an unnamed sibling.
///
/// Instead, this function reads the committed decision back and confirms the
/// commit's actual diff matches it exactly -- no more, no less.
/// [`ReleaseDecisionV1`]'s own deserializer already rejects a decision whose
/// entries don't match its content digest, so a hand-edited or corrupted
/// decision file fails before this function's own diff cross-check runs.
///
/// The caller must check out the exact merge commit in detached HEAD state
/// before creating an intent. GitHub-specific PR/approval provenance belongs
/// in the workflow boundary; this graph function verifies the local,
/// provider-neutral commit delta only.
pub fn derive_release_commit_decision<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    release_commit: &CommitSha,
    decision_path: &std::path::Path,
) -> Result<ReleaseDecisionV1, GraphError> {
    let head = git_stdout(workspace.runner, &workspace.root, &["rev-parse", "HEAD"])?;
    let head = CommitSha::parse(&head).map_err(|_error| GraphError::ReleaseIntentStale)?;
    if &head != release_commit {
        return Err(GraphError::ReleaseIntentStale);
    }

    let parent_ref = format!("{}^", release_commit.as_str());
    let parent = git_stdout(workspace.runner, &workspace.root, &["rev-parse", &parent_ref])?;
    CommitSha::parse(&parent).map_err(|_error| GraphError::ReleaseIntentStale)?;

    let changed = git_stdout(
        workspace.runner,
        &workspace.root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-r",
            "--no-renames",
            &parent,
            release_commit.as_str(),
        ],
    )?;
    let changed = parse_name_status(&changed)?;

    // The decision must be freshly authored as part of *this* commit, not a
    // stale leftover an earlier release already committed and this one never
    // touched -- otherwise a commit that changes nothing real could still
    // carry forward a prior, unrelated decision's authority.
    let decision_path_str = decision_path.to_string_lossy().replace('\\', "/");
    let decision_freshly_written = changed
        .iter()
        .any(|(status, path)| path == &decision_path_str && matches!(status.as_str(), "A" | "M"));
    if !decision_freshly_written {
        return Err(GraphError::ReleaseIntentStale);
    }

    // A cheap, independent sanity check that this commit is a real version
    // application and not just a hand-crafted decision file: some real
    // changeset was actually consumed. The decision's own entries, not this
    // changeset's content, remain the sole authority verified below.
    let changeset_dir = workspace.config.changesets_dir.to_string_lossy().replace('\\', "/");
    let changeset_prefix = format!("{}/", changeset_dir.trim_end_matches('/'));
    let consumed_a_changeset = changed
        .iter()
        .any(|(status, path)| status == "D" && path.starts_with(&changeset_prefix) && path.ends_with(".md"));
    if !consumed_a_changeset {
        return Err(GraphError::ReleaseIntentStale);
    }

    let decision_source = git_file(
        workspace.runner,
        &workspace.root,
        release_commit.as_str(),
        &decision_path_str,
    )?;
    let decision: ReleaseDecisionV1 =
        serde_json::from_str(&decision_source).map_err(|_error| GraphError::ReleaseIntentStale)?;
    let claimed = decision
        .entries
        .iter()
        .map(|entry| (entry.package.clone(), entry.target_version.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();

    let changed_paths = changed
        .iter()
        .filter_map(|(status, path)| matches!(status.as_str(), "A" | "M").then_some(path.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    let mut observed = std::collections::BTreeSet::new();
    for package in workspace.graph.packages() {
        let package_ids = release_package_ids(package)?;
        let package_is_claimed = package_ids.iter().any(|id| claimed.contains_key(id));
        if package_is_claimed {
            let changelog = package.changelog.as_ref().ok_or(GraphError::ReleaseIntentStale)?;
            if !changed_paths.contains(changelog.to_string_lossy().as_ref()) {
                return Err(GraphError::ReleaseIntentStale);
            }
        }
        for (manifest, id) in package.canonical_manifests().zip(package_ids) {
            let path = manifest.path.to_string_lossy();
            let before = manifest_version_at(workspace.runner, &workspace.root, &parent, &path, manifest.ecosystem())?;
            let after = manifest_version_at(
                workspace.runner,
                &workspace.root,
                release_commit.as_str(),
                &path,
                manifest.ecosystem(),
            )?;
            let changed_version = before != after;
            match claimed.get(&id) {
                Some(target_version) => {
                    if !changed_version || &after != target_version || !changed_paths.contains(path.as_ref()) {
                        return Err(GraphError::ReleaseIntentStale);
                    }
                    observed.insert(id);
                }
                None => {
                    if changed_version {
                        // A changed version the committed decision never
                        // claimed is not this commit's authority -- fail
                        // closed rather than trust a partial match.
                        return Err(GraphError::ReleaseIntentStale);
                    }
                }
            }
        }
    }
    if observed.len() != claimed.len() {
        return Err(GraphError::ReleaseIntentStale);
    }

    Ok(decision)
}

fn git_stdout<R: CommandRunner>(runner: &R, root: &std::path::Path, args: &[&str]) -> Result<String, GraphError> {
    let output = runner.run("git", args, root)?;
    if !output.success() {
        return Err(GraphError::ReleaseIntentStale);
    }
    Ok(output.stdout_trimmed().to_string())
}

fn parse_name_status(output: &str) -> Result<Vec<(String, String)>, GraphError> {
    output
        .lines()
        .map(|line| {
            let (status, path) = line.split_once('\t').ok_or(GraphError::ReleaseIntentStale)?;
            if !matches!(status, "A" | "M" | "D") || path.is_empty() || path.contains('\0') {
                return Err(GraphError::ReleaseIntentStale);
            }
            Ok((status.to_string(), path.to_string()))
        })
        .collect()
}

fn git_file<R: CommandRunner>(
    runner: &R,
    root: &std::path::Path,
    commit: &str,
    path: &str,
) -> Result<String, GraphError> {
    let object = format!("{commit}:{path}");
    git_stdout(runner, root, &["show", &object])
}

fn manifest_version_at<R: CommandRunner>(
    runner: &R,
    root: &std::path::Path,
    commit: &str,
    path: &str,
    ecosystem: Ecosystem,
) -> Result<Version, GraphError> {
    let source = git_file(runner, root, commit, path)?;
    let format = ecosystem
        .canonical_manifest_format()
        .ok_or(GraphError::ReleaseIntentStale)?;
    // A malformed blob, a version-less manifest (e.g. a Cargo workspace
    // root with no [package] table, or a `dynamic = ["version"]` PEP 621
    // package), or a Cargo `version.workspace = true` inheritance (which
    // `read_identity` deliberately never resolves, having no workspace
    // context for a historical git blob) all fail closed here rather than
    // panicking or fabricating a version.
    let version = callisto_manifests::read_identity(format, &source, std::path::Path::new(path))
        .ok()
        .and_then(|identity| match identity.version {
            Some(callisto_manifests::VersionSource::Literal(v)) => Some(v),
            _ => None,
        })
        .ok_or(GraphError::ReleaseIntentStale)?;
    Version::parse(&version, ecosystem.version_grammar()).map_err(|_error| GraphError::ReleaseIntentStale)
}

fn reason_from_bump(
    reason: Option<&BumpReason>,
    package_ids: &std::collections::BTreeMap<callisto_model::PackageId, Vec<ReleasePackageId>>,
) -> Result<ReleaseInclusionReason, GraphError> {
    match reason {
        Some(BumpReason::Changeset { .. }) | None => Ok(ReleaseInclusionReason::Changeset),
        Some(BumpReason::Inference { .. }) => Ok(ReleaseInclusionReason::Inference),
        Some(BumpReason::LinkedGroupUnion { group }) => Ok(ReleaseInclusionReason::LinkedGroup {
            group_id: group.to_string(),
        }),
        Some(BumpReason::FixedGroupUnion { group } | BumpReason::NewGroupMember { group }) => {
            Ok(ReleaseInclusionReason::FixedGroup {
                group_id: group.to_string(),
            })
        }
        Some(BumpReason::PreRelease { tag }) => Ok(ReleaseInclusionReason::PreReleasePolicy { policy_id: tag.clone() }),
        Some(BumpReason::Cascade { via, dep_kind, .. }) => {
            let source = package_ids
                .get(via)
                .and_then(|ids| (ids.len() == 1).then(|| ids[0].clone()))
                .ok_or(GraphError::ReleaseIntentStale)?;
            Ok(ReleaseInclusionReason::Cascade {
                from: source,
                edge_kind: format!("{dep_kind:?}"),
            })
        }
        Some(BumpReason::PeerEscalation { via, .. }) => {
            let source = package_ids
                .get(via)
                .and_then(|ids| (ids.len() == 1).then(|| ids[0].clone()))
                .ok_or(GraphError::ReleaseIntentStale)?;
            Ok(ReleaseInclusionReason::Cascade {
                from: source,
                edge_kind: "peer".to_string(),
            })
        }
        Some(_) => Err(GraphError::ReleaseIntentStale),
    }
}

#[cfg(test)]
mod tests {
    use callisto_model::{Ecosystem, Version};

    use super::*;

    #[test]
    fn decision_is_canonical_and_ecosystem_qualified() {
        let cargo = ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap();
        let npm = ReleasePackageId::new(Ecosystem::Npm, "demo").unwrap();
        let first = ReleaseDecisionV1::new(vec![
            ReleaseDecisionEntry {
                package: npm,
                target_version: Version::semver(1, 0, 0),
                reasons: vec![ReleaseInclusionReason::ExplicitSelection],
            },
            ReleaseDecisionEntry {
                package: cargo,
                target_version: Version::semver(1, 0, 0),
                reasons: vec![ReleaseInclusionReason::ExplicitSelection],
            },
        ])
        .unwrap();
        assert_eq!(first.entries[0].package.to_string(), "cargo/demo");
        assert_ne!(first.entries[0].package, first.entries[1].package);
    }

    #[test]
    fn release_commit_delta_parser_accepts_only_unambiguous_path_statuses() {
        assert_eq!(
            parse_name_status("M\tCargo.toml\nD\t.changeset/release.md\n").unwrap(),
            vec![
                ("M".to_string(), "Cargo.toml".to_string()),
                ("D".to_string(), ".changeset/release.md".to_string()),
            ]
        );
        assert!(parse_name_status("R100\told\tnew\n").is_err());
        assert!(parse_name_status("M Cargo.toml\n").is_err());
    }

    struct FixedBlobRunner(&'static str);

    impl CommandRunner for FixedBlobRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            Ok(callisto_model::CommandOutput {
                exit_code: Some(0),
                stdout: self.0.to_string(),
                stderr: String::new(),
            })
        }
    }

    // toml_edit's `Index` impl panics on a missing table ("index not found")
    // rather than returning None -- manifest_version_at previously used it
    // directly (`document["project"]["version"]`), so a Poetry-only
    // pyproject.toml, a PEP 621 `dynamic = ["version"]` package, or a
    // `Cargo.toml` with no `[package]` table would abort release-commit
    // verification with a panic instead of failing closed.
    #[test]
    fn manifest_version_at_fails_closed_instead_of_panicking_on_missing_tables() {
        let cases: &[(&str, Ecosystem)] = &[
            ("[project]\nname = \"x\"\ndynamic = [\"version\"]\n", Ecosystem::Pypi),
            ("[workspace]\nmembers = [\"a\"]\n", Ecosystem::Cargo),
        ];
        for (source, ecosystem) in cases {
            let runner = FixedBlobRunner(source);
            let result = manifest_version_at(&runner, std::path::Path::new("."), "deadbeef", "Cargo.toml", *ecosystem);
            assert!(
                matches!(result, Err(GraphError::ReleaseIntentStale)),
                "expected a clean ReleaseIntentStale rejection (not a panic) for {ecosystem:?}, got {result:?}"
            );
        }
    }

    #[test]
    fn manifest_version_at_still_resolves_poetry_fallback_and_normal_cargo() {
        let poetry = FixedBlobRunner("[tool.poetry]\nname = \"x\"\nversion = \"1.0.0\"\n");
        assert_eq!(
            manifest_version_at(
                &poetry,
                std::path::Path::new("."),
                "deadbeef",
                "pyproject.toml",
                Ecosystem::Pypi
            )
            .unwrap(),
            Version::parse("1.0.0", Ecosystem::Pypi.version_grammar()).unwrap()
        );

        let cargo = FixedBlobRunner("[package]\nname = \"x\"\nversion = \"1.2.3\"\n");
        assert_eq!(
            manifest_version_at(
                &cargo,
                std::path::Path::new("."),
                "deadbeef",
                "Cargo.toml",
                Ecosystem::Cargo
            )
            .unwrap(),
            Version::semver(1, 2, 3)
        );
    }
}
