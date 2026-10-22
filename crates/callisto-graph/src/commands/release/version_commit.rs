//! The commit that set a package's version, so a rerun after a half-finished release cannot tag later code.

use std::collections::BTreeSet;

use callisto_manifests::{read_identity, VersionSource};
use callisto_model::vcs::GitAccess;
use callisto_model::{ManifestDecl, ReleaseDecisionV1, ReleasePackageId};

use crate::changed::package_paths;
use crate::{DependencyResolver, GraphError, Workspace};

/// How many commits touching the manifest are read looking for the one that set the version.
const HISTORY_LIMIT: usize = 100;

/// A package that changed after the commit that set the version it is about to release.
pub(super) struct MovedPast {
    pub package: ReleasePackageId,
    pub version: String,
    pub version_commit: String,
    pub newer_commits: usize,
}

/// The selected packages with commits touching their files after the commit that set their release version.
///
/// A package whose version commit cannot be found (an inherited or unreadable version, or a long history) is not
/// reported: the check only ever refuses on evidence. `resumable` packages are tagged on HEAD already.
pub(super) fn packages_moved_past_their_version<R: callisto_model::CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    resumable: &BTreeSet<ReleasePackageId>,
) -> Result<Vec<MovedPast>, GraphError> {
    let git = workspace.git_access();
    let mut moved = Vec::new();
    for package in workspace.graph.packages() {
        let ids = crate::commands::release_decision::release_package_ids(&workspace.identity, package)?;
        let Some(entry) = decision
            .entries
            .iter()
            .find(|entry| ids.contains(&entry.package) && !resumable.contains(&entry.package))
        else {
            continue;
        };
        let Some(manifest) = package.canonical_manifests().next() else {
            continue;
        };
        let version = entry.target_version.render().to_owned();
        let Some(version_commit) = commit_setting_version(git, manifest, &version)? else {
            continue;
        };
        let newer_commits = git.commits_since(Some(&version_commit), &package_paths(package))?.len();
        if newer_commits > 0 {
            moved.push(MovedPast {
                package: entry.package.clone(),
                version,
                version_commit,
                newer_commits,
            });
        }
    }
    Ok(moved)
}

/// The newest commit whose manifest holds `version` while its parent's does not.
fn commit_setting_version(
    git: &GitAccess<'_>,
    manifest: &ManifestDecl,
    version: &str,
) -> Result<Option<String>, GraphError> {
    for commit in git
        .commits_since(None, std::slice::from_ref(&manifest.path))?
        .iter()
        .take(HISTORY_LIMIT)
    {
        let sha = commit.sha.as_str();
        if version_at(git, sha, manifest)?.as_deref() != Some(version) {
            continue;
        }
        // A root commit has no parent, so the version was set there.
        let parent = version_at(git, &format!("{sha}^"), manifest).unwrap_or(None);
        if parent.as_deref() != Some(version) {
            return Ok(Some(sha.to_owned()));
        }
    }
    Ok(None)
}

/// The literal version `manifest` declared at `rev`, if it held one.
fn version_at(git: &GitAccess<'_>, rev: &str, manifest: &ManifestDecl) -> Result<Option<String>, GraphError> {
    let Some(text) = git.read_at(rev, &manifest.path)? else {
        return Ok(None);
    };
    Ok(
        match read_identity(manifest.format, &text, &manifest.path)
            .ok()
            .and_then(|identity| identity.version)
        {
            Some(VersionSource::Literal(version)) => Some(version),
            _ => None,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::commands::release::tests::{fixture, RealGitRunner};
    use crate::IgnoreWalkLocator;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn moved(root: &Path) -> Vec<(String, String)> {
        let workspace = Workspace::load(root.to_path_buf(), &IgnoreWalkLocator::new(root), &RealGitRunner).unwrap();
        let decision = crate::commands::release_decision::derive_release_roster(&workspace, &[], &BTreeSet::new())
            .unwrap()
            .expect("unreleased packages");
        packages_moved_past_their_version(&workspace, &decision, &BTreeSet::new())
            .unwrap()
            .into_iter()
            .map(|moved| (moved.package.to_string(), moved.version_commit))
            .collect()
    }

    #[test]
    fn a_package_untouched_since_its_version_commit_is_not_reported() {
        let (dir, _) = fixture();
        assert!(moved(dir.path()).is_empty());
    }

    #[test]
    fn a_later_commit_in_the_package_is_reported_with_the_version_commit() {
        let (dir, _) = fixture();
        let version_commit = git(dir.path(), &["rev-parse", "HEAD"]);
        std::fs::write(dir.path().join("NOTES.md"), "later\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "later"]);
        assert_eq!(
            moved(dir.path()),
            vec![("cargo/release-fixture".to_owned(), version_commit)]
        );
    }

    #[test]
    fn a_commit_in_another_package_is_not_a_move() {
        let (dir, _) = fixture();
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        for name in ["a", "b"] {
            std::fs::create_dir_all(root.join(format!("crates/{name}/src"))).unwrap();
            std::fs::write(
                root.join(format!("crates/{name}/Cargo.toml")),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2024\"\n"),
            )
            .unwrap();
            std::fs::write(root.join(format!("crates/{name}/src/lib.rs")), "\n").unwrap();
        }
        std::fs::write(root.join("callisto.toml"), "").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "workspace"]);
        let version_commit = git(root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("crates/b/src/lib.rs"), "pub fn b() {}\n").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "change b"]);
        assert_eq!(moved(root), vec![("cargo/b".to_owned(), version_commit)]);
    }
}
