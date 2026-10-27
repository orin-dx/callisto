//! Merged release commits whose release never finished.
//!
//! A release publishes to the registries first, creates a package's tag only after they confirm, and publishes its
//! GitHub release last. So a release is finished exactly when every package in its committed decision has its tag and
//! every package released to GitHub has a published release. Both are cheap to read, which lets this look at old
//! release commits without planning from them.

use std::collections::BTreeSet;
use std::path::Path;

use callisto_model::{CommandRunner, CommitSha, PublishTarget, ReleaseDecisionV1, ReleasePackageId, TagName, Version};

use super::github::{github_release_for_tag, GitHubReleaseLookup};
use super::provider::ProviderContext;
use crate::commands::release_decision::{has_dispatchable_target, tag_of_version, workspace_package};
use crate::{DependencyResolver, GraphError, TagIndex, Workspace};

/// A merged release the committed decision describes that has not finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRelease {
    /// The release commit: the source to release.
    pub source: CommitSha,
    pub packages: Vec<(ReleasePackageId, Version)>,
    /// What the release should have produced and did not: tags, and GitHub releases that are missing or still drafts.
    pub missing: Vec<String>,
}

/// The unfinished releases among the last `limit` commits on HEAD's first-parent line that wrote the decision at
/// `decision_path`, oldest first. Scanning newest to oldest stops at the first finished release, since releases are
/// sequential and anything older was released before it. With `from`, only that release commit and newer ones count.
pub fn pending_releases<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision_path: &Path,
    limit: usize,
    from: Option<&CommitSha>,
) -> Result<Vec<PendingRelease>, GraphError> {
    let git = workspace.git_access();
    let mut commits = git.first_parent_commits_touching(&[decision_path.to_path_buf()])?;
    if let Some(from) = from {
        let at = commits
            .iter()
            .position(|commit| commit == from.as_str())
            .ok_or_else(|| GraphError::ReleaseInvariant {
                detail: format!("`{}` is not a release commit on this branch", from.as_str()),
            })?;
        commits.truncate(at + 1);
    }

    let tags = workspace.tags()?;
    let forge = workspace
        .config
        .product_release
        .as_ref()
        .and_then(|release| release.forge_repository.as_ref())
        .map(|repository| format!("{}/{}", repository.owner(), repository.repo()));
    let context = ProviderContext::new(&workspace.root, workspace.runner, None);

    let mut pending = Vec::new();
    for commit in commits.into_iter().take(limit) {
        let source = CommitSha::parse(&commit).map_err(|error| GraphError::ReleaseInvariant {
            detail: format!("git named the release commit `{commit}`: {error}"),
        })?;
        let Some(text) = git.read_at(&commit, decision_path)? else {
            continue;
        };
        let decision: ReleaseDecisionV1 =
            serde_json::from_str(&text).map_err(|error| GraphError::ReleaseDecisionDecode {
                path: decision_path.to_path_buf(),
                message: error.to_string(),
            })?;
        let missing = release_gaps(workspace, &context, tags, forge.as_deref(), &decision);
        if missing.is_empty() {
            break;
        }
        pending.push(PendingRelease {
            source,
            packages: decision
                .entries
                .iter()
                .map(|entry| (entry.package.clone(), entry.target_version.clone()))
                .collect(),
            missing,
        });
    }
    pending.reverse();
    Ok(pending)
}

/// What `decision`'s release should have produced and has not.
fn release_gaps<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    context: &ProviderContext<'_>,
    tags: &TagIndex,
    forge: Option<&str>,
    decision: &ReleaseDecisionV1,
) -> Vec<String> {
    let mut gaps = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in &decision.entries {
        // A package that is gone, or no longer published anywhere, cannot be released now.
        let Some(package) = workspace_package(workspace, &entry.package) else {
            continue;
        };
        if !has_dispatchable_target(package) {
            continue;
        }
        let expected = tags.template(&package.id).render(&entry.target_version).to_string();
        // A package with several identities has one tag.
        if !seen.insert(expected.clone()) {
            continue;
        }
        let Some(tag) = tag_of_version(tags, &package.id, &entry.target_version) else {
            gaps.push(format!("tag {expected}"));
            continue;
        };
        if package.publish_to.contains(&PublishTarget::GitHubRelease)
            && let (Some(repository), Ok(name)) = (forge, TagName::parse(&tag))
            && !release_is_published(workspace, context, repository, &name)
        {
            gaps.push(format!("published GitHub release for {tag}"));
        }
    }
    gaps
}

/// A release that cannot be read counts as not published, so the release is rerun: that adopts what already landed.
fn release_is_published<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    context: &ProviderContext<'_>,
    repository: &str,
    tag: &TagName,
) -> bool {
    matches!(
        github_release_for_tag(&workspace.root, workspace.runner, context.sleeper(), repository, tag),
        Ok(GitHubReleaseLookup::Found(value)) if value.get("draft").and_then(serde_json::Value::as_bool) != Some(true)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::release::tests::{fixture, RealGitRunner};
    use crate::IgnoreWalkLocator;
    use callisto_model::{Ecosystem, ReleaseDecisionEntry, ReleaseInclusionReason, VersionGrammar};

    const DECISION: &str = ".callisto/release-decision.json";

    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// Commits a decision releasing `version` and returns that commit.
    fn release_commit(root: &Path, version: &str) -> CommitSha {
        let decision = ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
            package: ReleasePackageId::new(Ecosystem::Cargo, "release-fixture").unwrap(),
            target_version: Version::parse(version, VersionGrammar::SemVer).unwrap(),
            reasons: vec![ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap();
        std::fs::create_dir_all(root.join(".callisto")).unwrap();
        std::fs::write(root.join(DECISION), serde_json::to_string_pretty(&decision).unwrap()).unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", &format!("release {version}")]);
        CommitSha::parse(&git(root, &["rev-parse", "HEAD"])).unwrap()
    }

    fn tag(root: &Path, version: &str) {
        git(
            root,
            &["tag", "-a", "-m", "release", &format!("release-fixture@{version}")],
        );
    }

    fn pending(root: &Path, limit: usize, from: Option<&CommitSha>) -> Result<Vec<PendingRelease>, GraphError> {
        let workspace = Workspace::load(root.to_path_buf(), &IgnoreWalkLocator::new(root), &RealGitRunner)?;
        pending_releases(&workspace, Path::new(DECISION), limit, from)
    }

    fn sources(found: &[PendingRelease]) -> Vec<&CommitSha> {
        found.iter().map(|release| &release.source).collect()
    }

    #[test]
    fn a_release_whose_tags_all_exist_is_finished() {
        let (dir, _) = fixture();
        release_commit(dir.path(), "1.0.0");
        tag(dir.path(), "1.0.0");
        assert!(pending(dir.path(), 10, None).unwrap().is_empty());
    }

    #[test]
    fn a_release_commit_with_no_tag_is_pending_and_names_what_is_missing() {
        let (dir, _) = fixture();
        let commit = release_commit(dir.path(), "1.0.0");
        let found = pending(dir.path(), 10, None).unwrap();
        assert_eq!(sources(&found), [&commit]);
        assert_eq!(found[0].missing, ["tag release-fixture@1.0.0"]);
        assert_eq!(found[0].packages.len(), 1);
    }

    #[test]
    fn pending_releases_come_oldest_first_and_stop_at_the_newest_finished_one() {
        let (dir, _) = fixture();
        let root = dir.path();
        release_commit(root, "1.0.0");
        tag(root, "1.0.0");
        let second = release_commit(root, "1.1.0");
        let third = release_commit(root, "1.2.0");
        assert_eq!(sources(&pending(root, 10, None).unwrap()), [&second, &third]);

        // Releases are sequential: an unfinished one older than a finished one was released before it.
        tag(root, "1.2.0");
        assert!(pending(root, 10, None).unwrap().is_empty());
    }

    #[test]
    fn from_starts_at_a_release_commit_and_an_unknown_one_is_an_error() {
        let (dir, _) = fixture();
        let root = dir.path();
        release_commit(root, "1.0.0");
        let second = release_commit(root, "1.1.0");
        let third = release_commit(root, "1.2.0");
        assert_eq!(sources(&pending(root, 10, Some(&second)).unwrap()), [&second, &third]);
        assert_eq!(sources(&pending(root, 10, Some(&third)).unwrap()), [&third]);

        let elsewhere = CommitSha::parse(&"a".repeat(40)).unwrap();
        let error = pending(root, 10, Some(&elsewhere)).unwrap_err();
        assert!(error.to_string().contains("not a release commit"), "{error}");
    }

    #[test]
    fn only_the_last_limit_release_commits_are_considered() {
        let (dir, _) = fixture();
        let root = dir.path();
        release_commit(root, "1.0.0");
        release_commit(root, "1.1.0");
        let newest = release_commit(root, "1.2.0");
        assert_eq!(sources(&pending(root, 1, None).unwrap()), [&newest]);
    }

    #[test]
    fn a_repository_with_no_release_commits_has_nothing_pending() {
        let (dir, _) = fixture();
        assert!(pending(dir.path(), 10, None).unwrap().is_empty());
    }
}
