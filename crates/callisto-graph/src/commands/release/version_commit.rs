//! The commit that set a package's version, so a rerun after a half-finished release cannot tag later code.

use std::cell::RefCell;
use std::collections::BTreeSet;

use callisto_manifests::{read_effective_version, WorkspaceReads};
use callisto_model::vcs::{GitAccess, VcsError};
use callisto_model::{ManifestDecl, ManifestError, ReleaseDecisionV1, ReleasePackageId, Version};

use crate::changed::package_paths;
use crate::{DependencyResolver, GraphError, Workspace};

/// A package whose files differ from the commit that set the version it is about to release, or whose version
/// commit could not be found.
pub(super) struct MovedPast {
    pub package: ReleasePackageId,
    pub version: String,
    /// `None` when no commit on the first-parent history sets this version.
    pub version_commit: Option<String>,
}

/// The selected packages that differ from the commit that set their release version, or whose version commit is
/// unknown. `resumable` packages are tagged on HEAD already.
///
/// A package is compared by tree content, so a revert or a change made only inside a merge commit counts the same as
/// a plain commit. The caller refuses only the ones whose version the registry already holds.
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
        // A manifest that is not committed has no history to compare; the clean-tree rule covers it.
        let Some(head) = effective_version_at(git, "HEAD", manifest)? else {
            continue;
        };
        let version_commit = commit_setting_version(git, manifest, &head, &entry.target_version)?;
        let unchanged = match &version_commit {
            Some(commit) => git.same_content_at(commit, "HEAD", &package_paths(package))?,
            None => false,
        };
        if !unchanged {
            moved.push(MovedPast {
                package: entry.package.clone(),
                version: entry.target_version.render().to_owned(),
                version_commit,
            });
        }
    }
    Ok(moved)
}

/// The newest first-parent commit whose manifest holds `version` while its parent's does not.
///
/// Only the manifest and, for an inherited version, the workspace manifest that declares it can set the version, so
/// those are the only paths walked. A history with no such commit gives `None`, never a guess.
fn commit_setting_version(
    git: &GitAccess<'_>,
    manifest: &ManifestDecl,
    head: &callisto_manifests::EffectiveManifestVersion,
    version: &Version,
) -> Result<Option<String>, GraphError> {
    let mut paths = vec![manifest.path.clone()];
    if head.authority != manifest.path {
        paths.push(head.authority.clone());
    }
    for commit in git.first_parent_commits_touching(&paths)? {
        if effective_version_at(git, &commit, manifest)?
            .map(|found| found.version)
            .as_ref()
            != Some(version)
        {
            continue;
        }
        // A root commit has no parent, so the version was set there.
        let parent = effective_version_at(git, &format!("{commit}^"), manifest)?;
        if parent.map(|found| found.version).as_ref() != Some(version) {
            return Ok(Some(commit));
        }
    }
    Ok(None)
}

/// The version `manifest` holds at `rev`, resolving a workspace-inherited one. A manifest that is absent, does not
/// parse or declares no version there gives `None`; a failing git read is an error.
fn effective_version_at(
    git: &GitAccess<'_>,
    rev: &str,
    manifest: &ManifestDecl,
) -> Result<Option<callisto_manifests::EffectiveManifestVersion>, GraphError> {
    // `read_effective_version` reports manifest errors only, so a git failure is held here and returned after.
    let git_failure: RefCell<Option<VcsError>> = RefCell::new(None);
    let mut read = |path: &std::path::Path| -> Result<Option<String>, ManifestError> {
        Ok(git.read_at(rev, path).unwrap_or_else(|error| {
            git_failure.borrow_mut().get_or_insert(error);
            None
        }))
    };
    let found = match read(&manifest.path)? {
        Some(source) => read_effective_version(
            manifest.format,
            &source,
            &manifest.path,
            &mut WorkspaceReads::default(),
            &mut read,
        )
        .ok()
        .flatten(),
        None => None,
    };
    match git_failure.into_inner() {
        Some(error) => Err(error.into()),
        None => Ok(found),
    }
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

    fn commit_all(root: &Path, message: &str) -> String {
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", message]);
        git(root, &["rev-parse", "HEAD"])
    }

    fn moved(root: &Path) -> Vec<(String, Option<String>)> {
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

    /// A workspace whose members `a` and `b` inherit `1.0.0` from the root, committed once.
    fn inheriting_workspace(root: &Path) -> String {
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"3\"\n\n[workspace.package]\nversion = \"1.0.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        for name in ["a", "b"] {
            std::fs::create_dir_all(root.join(format!("crates/{name}/src"))).unwrap();
            std::fs::write(
                root.join(format!("crates/{name}/Cargo.toml")),
                format!("[package]\nname = \"{name}\"\nversion.workspace = true\nedition.workspace = true\n"),
            )
            .unwrap();
            std::fs::write(root.join(format!("crates/{name}/src/lib.rs")), "\n").unwrap();
        }
        std::fs::write(root.join("callisto.toml"), "").unwrap();
        commit_all(root, "workspace")
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
        commit_all(dir.path(), "later");
        assert_eq!(
            moved(dir.path()),
            vec![("cargo/release-fixture".to_owned(), Some(version_commit))]
        );
    }

    #[test]
    fn a_commit_in_another_package_is_not_a_move() {
        let (dir, _) = fixture();
        let root = dir.path();
        let version_commit = inheriting_workspace(root);
        std::fs::write(root.join("crates/b/src/lib.rs"), "pub fn b() {}\n").unwrap();
        commit_all(root, "change b");
        assert_eq!(moved(root), vec![("cargo/b".to_owned(), Some(version_commit))]);
    }

    #[test]
    fn an_inherited_version_is_located_at_the_workspace_manifest_that_set_it() {
        let (dir, _) = fixture();
        let root = dir.path();
        let version_commit = inheriting_workspace(root);
        std::fs::write(root.join("crates/a/src/lib.rs"), "pub fn a() {}\n").unwrap();
        commit_all(root, "change a");
        assert_eq!(
            moved(root),
            vec![("cargo/a".to_owned(), Some(version_commit))],
            "a package that inherits its version is checked like any other"
        );
    }

    #[test]
    fn a_version_set_long_ago_is_still_found() {
        let (dir, _) = fixture();
        let root = dir.path();
        let version_commit = git(root, &["rev-parse", "HEAD"]);
        let manifest = root.join("Cargo.toml");
        let original = std::fs::read_to_string(&manifest).unwrap();
        for index in 0..105 {
            std::fs::write(&manifest, format!("{original}\n# edit {index}\n")).unwrap();
            commit_all(root, "touch the manifest");
        }
        assert_eq!(
            moved(root),
            vec![("cargo/release-fixture".to_owned(), Some(version_commit))],
            "more than a hundred manifest edits above the version commit must not hide it"
        );
    }

    #[test]
    fn a_release_branch_merged_after_unrelated_trunk_commits_is_not_a_move() {
        let (dir, _) = fixture();
        let root = dir.path();
        git(root, &["switch", "-q", "-c", "trunk"]);
        git(root, &["switch", "-q", "-c", "release"]);
        let manifest = root.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(&manifest, text.replace("1.2.3", "1.3.0")).unwrap();
        commit_all(root, "release 1.3.0");
        git(root, &["switch", "-q", "trunk"]);
        std::fs::write(root.join("NOTES.md"), "trunk moved on\n").unwrap();
        commit_all(root, "trunk moved on");
        git(root, &["merge", "-q", "--no-ff", "-m", "merge the release", "release"]);
        assert!(
            moved(root).is_empty(),
            "HEAD is the merge that landed the version, so nothing differs from the commit that set it"
        );
    }

    #[test]
    fn a_version_no_commit_set_is_reported_without_a_commit() {
        let (dir, _) = fixture();
        let root = dir.path();
        let manifest = root.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        let bumped = text.replacen("version = \"", "version = \"9", 1);
        std::fs::write(&manifest, bumped).unwrap();
        // Uncommitted: no commit holds this version, so the guard cannot say it is unchanged.
        assert!(moved(root).iter().all(|(_, commit)| commit.is_none()));
    }
}
