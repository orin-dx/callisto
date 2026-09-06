//! Deterministic, graph-owned release roster decisions.
//!
//! This is deliberately separate from `PublishPlan`: it records the exact
//! package/version authority that later intent construction consumes, without
//! exposing a mutation route.

use callisto_model::{
    BumpReason, CommandRunner, CommitSha, Ecosystem, ReleaseDecisionEntry, ReleaseDecisionV1, ReleaseInclusionReason,
    ReleasePackageId, Version,
};

use crate::{DependencyResolver, GraphError, VersionPlan, Workspace};

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
        let ids = package
            .canonical_manifests()
            .map(|manifest| ReleasePackageId::new(manifest.ecosystem(), package.id.name()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_error| GraphError::ReleaseIntentStale)?;
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

/// Derives the release roster from a merged release commit already checked out
/// at `release_commit`.
///
/// This is intentionally *not* a second call to [`crate::commands::plan_version`].
/// The release PR has already used its pending changesets to calculate and
/// apply the version plan.  At merge time those changesets are gone, and the
/// immutable merge commit is the authority.  We therefore require that it
/// deletes its consumed changesets, and the canonical manifest versions and
/// configured changelogs form exactly the resulting roster.
///
/// The caller must check out the exact merge commit in detached HEAD state
/// before creating an intent.  GitHub-specific PR/approval provenance belongs
/// in the workflow boundary; this graph function verifies the local,
/// provider-neutral commit delta only.
pub fn derive_release_commit_decision<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    release_commit: &CommitSha,
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
    let changeset_dir = workspace.config.changesets_dir.to_string_lossy().replace('\\', "/");
    let changeset_prefix = format!("{}/", changeset_dir.trim_end_matches('/'));
    let deleted_changesets = changed
        .iter()
        .filter_map(|(status, path)| {
            (status == "D" && path.starts_with(&changeset_prefix) && path.ends_with(".md")).then_some(path)
        })
        .collect::<Vec<_>>();
    if deleted_changesets.is_empty() {
        return Err(GraphError::ReleaseIntentStale);
    }

    let mut selected = std::collections::BTreeSet::new();
    for path in deleted_changesets {
        let source = git_file(workspace.runner, &workspace.root, &parent, path)?;
        let changeset = callisto_format::parse_changeset(&source).map_err(|_error| GraphError::ReleaseIntentStale)?;
        for entry in changeset.entries {
            selected.extend(resolve_changeset_entry(workspace, &entry.name)?);
        }
    }

    // A changeset only ever names a bare package; it cannot select a whole
    // fixed/linked group, yet `solve_cascade` sweeps every group member to
    // the same converged target the moment one member bumps (Track 1). A
    // changeset-selected package's group siblings are therefore just as
    // authorized as the package itself -- expanding `selected` here, rather
    // than treating an unnamed sibling's version change as unreviewed, is
    // what makes this function agree with the decision `plan_version`
    // actually produced instead of a narrower one nothing ever generates.
    let groups = &workspace.config.groups;
    let mut triggered_groups = std::collections::BTreeSet::new();
    for package in workspace.graph.packages() {
        let is_selected = package.canonical_manifests().any(|manifest| {
            ReleasePackageId::new(manifest.ecosystem(), package.id.name()).is_ok_and(|id| selected.contains(&id))
        });
        if !is_selected {
            continue;
        }
        if let Some(group) = groups.fixed_of.get(&package.id) {
            triggered_groups.insert(group.clone());
        }
        if let Some(group) = groups.linked_of.get(&package.id) {
            triggered_groups.insert(group.clone());
        }
    }
    let mut authorized = selected.clone();
    if !triggered_groups.is_empty() {
        for package in workspace.graph.packages() {
            let in_triggered_group = groups
                .fixed_of
                .get(&package.id)
                .is_some_and(|group| triggered_groups.contains(group))
                || groups
                    .linked_of
                    .get(&package.id)
                    .is_some_and(|group| triggered_groups.contains(group));
            if !in_triggered_group {
                continue;
            }
            for manifest in package.canonical_manifests() {
                let id = ReleasePackageId::new(manifest.ecosystem(), package.id.name())
                    .map_err(|_error| GraphError::ReleaseIntentStale)?;
                authorized.insert(id);
            }
        }
    }

    let changed_paths = changed
        .iter()
        .filter_map(|(status, path)| matches!(status.as_str(), "A" | "M").then_some(path.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    let mut entries = Vec::new();
    let mut observed = std::collections::BTreeSet::new();
    let mut group_targets: std::collections::BTreeMap<
        &callisto_model::GroupName,
        Vec<(callisto_model::PackageId, Version)>,
    > = std::collections::BTreeMap::new();
    for package in workspace.graph.packages() {
        let package_ids = package
            .canonical_manifests()
            .map(|manifest| {
                ReleasePackageId::new(manifest.ecosystem(), package.id.name())
                    .map_err(|_error| GraphError::ReleaseIntentStale)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let package_is_authorized = package_ids.iter().any(|id| authorized.contains(id));
        if package_is_authorized {
            let changelog = package.changelog.as_ref().ok_or(GraphError::ReleaseIntentStale)?;
            if !changed_paths.contains(changelog.to_string_lossy().as_ref()) {
                return Err(GraphError::ReleaseIntentStale);
            }
        }
        for manifest in package.canonical_manifests() {
            let id = ReleasePackageId::new(manifest.ecosystem(), package.id.name())
                .map_err(|_error| GraphError::ReleaseIntentStale)?;
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
            if authorized.contains(&id) {
                if !changed_version || !changed_paths.contains(path.as_ref()) {
                    return Err(GraphError::ReleaseIntentStale);
                }
                observed.insert(id.clone());
                let reason = if selected.contains(&id) {
                    ReleaseInclusionReason::Changeset
                } else if let Some(group) = groups.fixed_of.get(&package.id) {
                    ReleaseInclusionReason::FixedGroup {
                        group_id: group.as_str().to_string(),
                    }
                } else if let Some(group) = groups.linked_of.get(&package.id) {
                    ReleaseInclusionReason::LinkedGroup {
                        group_id: group.as_str().to_string(),
                    }
                } else {
                    // Unreachable: `authorized` only ever gains an id via
                    // `selected` itself or via a `fixed_of`/`linked_of` hit
                    // above, so one of the three arms above always matches.
                    return Err(GraphError::ReleaseIntentStale);
                };
                if let Some(group) = groups
                    .fixed_of
                    .get(&package.id)
                    .or_else(|| groups.linked_of.get(&package.id))
                {
                    if triggered_groups.contains(group) {
                        group_targets
                            .entry(group)
                            .or_default()
                            .push((package.id.clone(), after.clone()));
                    }
                }
                entries.push(ReleaseDecisionEntry {
                    package: id,
                    target_version: after,
                    reasons: vec![reason],
                });
            } else if changed_version {
                // A changed version without a consumed changeset or a
                // triggered group cascade is an unreviewed extra release.
                // Dependency-driven `Cascade` and `PreReleasePolicy` bumps
                // remain unsupported here and still fail closed -- this
                // function only re-derives the two cascade shapes
                // (`FixedGroup`, `LinkedGroup`) that always converge every
                // member to one shared target, which is independently
                // checkable from the diff alone without re-running the full
                // dependency-graph cascade solver.
                return Err(GraphError::ReleaseIntentStale);
            }
        }
    }
    for (group, members) in &group_targets {
        let first = &members[0].1;
        if members[1..].iter().any(|(_, version)| version != first) {
            return Err(GraphError::ReleaseCommitCascadeDivergent {
                group: (*group).clone(),
                members: members.clone(),
            });
        }
    }
    if observed != authorized {
        return Err(GraphError::ReleaseIntentStale);
    }
    ReleaseDecisionV1::new(entries).map_err(|_error| GraphError::ReleaseIntentStale)
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
    let version = match ecosystem {
        Ecosystem::Cargo => source
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .and_then(|document| document["package"]["version"].as_str().map(str::to_owned)),
        Ecosystem::Npm => serde_json::from_str::<serde_json::Value>(&source)
            .ok()
            .and_then(|document| {
                document
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            }),
        Ecosystem::Pypi => source.parse::<toml_edit::DocumentMut>().ok().and_then(|document| {
            document["project"]["version"]
                .as_str()
                .or_else(|| document["tool"]["poetry"]["version"].as_str())
                .map(str::to_owned)
        }),
        _ => None,
    }
    .ok_or(GraphError::ReleaseIntentStale)?;
    Version::parse(&version, ecosystem.version_grammar()).map_err(|_error| GraphError::ReleaseIntentStale)
}

fn resolve_changeset_entry<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    raw: &str,
) -> Result<Vec<ReleasePackageId>, GraphError> {
    if raw.contains('/') {
        let id = ReleasePackageId::parse(raw).map_err(|_error| GraphError::ReleaseIntentStale)?;
        let known = workspace.graph.packages().any(|package| {
            package
                .canonical_manifests()
                .any(|manifest| manifest.ecosystem() == id.ecosystem() && package.id.name() == id.name())
        });
        return known.then_some(vec![id]).ok_or(GraphError::ReleaseIntentStale);
    }

    let packages = workspace
        .graph
        .packages()
        .filter(|package| package.id.name() == raw)
        .collect::<Vec<_>>();
    if packages.len() != 1 {
        return Err(GraphError::ReleaseIntentStale);
    }
    packages[0]
        .canonical_manifests()
        .map(|manifest| {
            ReleasePackageId::new(manifest.ecosystem(), packages[0].id.name())
                .map_err(|_error| GraphError::ReleaseIntentStale)
        })
        .collect()
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
}
