use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{workspace_relative, Ecosystem, PackageId, ProjectRoot};
use ignore::WalkBuilder;

use crate::locate::membership;
use crate::locate::{find_workspace_root, LocateError, ProjectLocator};

pub struct IgnoreWalkLocator {
    root: PathBuf,
    skip: BTreeSet<&'static str>,
}

impl IgnoreWalkLocator {
    pub fn new(root: &Path) -> Self {
        let mut skip = BTreeSet::new();
        skip.insert("target");
        skip.insert("node_modules");
        skip.insert(".git");
        skip.insert(".moon");
        skip.insert("dist");

        let canonical = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        IgnoreWalkLocator {
            root: canonical,
            skip,
        }
    }

    pub fn discover(start: &Path) -> Result<Self, LocateError> {
        let root = find_workspace_root(start)?;
        Ok(Self::new(&root))
    }
}

impl ProjectLocator for IgnoreWalkLocator {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError> {
        let mut results = Vec::new();
        let cargo_membership = membership::read_cargo_membership(&self.root);
        let walker = WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .parents(false)
            .max_depth(Some(32))
            .filter_entry({
                let skip = self.skip.clone();
                move |entry| {
                    if let Some(name) = entry.file_name().to_str() {
                        if skip.contains(name) {
                            return false;
                        }
                    }
                    true
                }
            })
            .build();

        for entry_res in walker {
            let entry = entry_res.map_err(|e| LocateError::Walk {
                path: self.root.clone(),
                message: e.to_string(),
            })?;

            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let cargo_toml = path.join("Cargo.toml");
            if cargo_toml.exists() {
                if let Ok(content) = fs::read_to_string(&cargo_toml) {
                    if content.contains("[package]") {
                        if let Ok(doc) = content.parse::<toml_edit::DocumentMut>() {
                            if let Some(name) = doc
                                .get("package")
                                .and_then(|p| p.get("name"))
                                .and_then(|n| n.as_str())
                            {
                                let rel = to_workspace_relative(path, &self.root)?;
                                let is_root = rel == Path::new(".");
                                if cargo_membership.admits(&rel, is_root) {
                                    let id = PackageId::parse(name)
                                        .unwrap_or_else(|_| PackageId::Bare(name.to_string()));
                                    results.push(ProjectRoot {
                                        id,
                                        path: rel,
                                        ecosystem: Ecosystem::Cargo,
                                    });
                                }
                            }
                        }
                    }
                }
            }

            let pkg_json = path.join("package.json");
            if pkg_json.exists() {
                if let Ok(content) = fs::read_to_string(&pkg_json) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(name) = val.get("name").and_then(|n| n.as_str()) {
                            let rel = to_workspace_relative(path, &self.root)?;
                            let id = PackageId::parse(name)
                                .unwrap_or_else(|_| PackageId::Bare(name.to_string()));
                            results.push(ProjectRoot {
                                id,
                                path: rel,
                                ecosystem: Ecosystem::Npm,
                            });
                        }
                    }
                }
            }

            let pyproject_toml = path.join("pyproject.toml");
            if pyproject_toml.exists() {
                if let Ok(content) = fs::read_to_string(&pyproject_toml) {
                    if let Ok(doc) = content.parse::<toml_edit::DocumentMut>() {
                        let name = doc
                            .get("project")
                            .and_then(|p| p.get("name"))
                            .and_then(|n| n.as_str())
                            .or_else(|| {
                                doc.get("tool")
                                    .and_then(|t| t.get("poetry"))
                                    .and_then(|p| p.get("name"))
                                    .and_then(|n| n.as_str())
                            });
                        if let Some(n) = name {
                            let rel = to_workspace_relative(path, &self.root)?;
                            let id = PackageId::parse(n)
                                .unwrap_or_else(|_| PackageId::Bare(n.to_string()));
                            results.push(ProjectRoot {
                                id,
                                path: rel,
                                ecosystem: Ecosystem::Pypi,
                            });
                        }
                    }
                }
            }
        }

        results.sort_by(|a, b| (&a.path, a.ecosystem).cmp(&(&b.path, b.ecosystem)));
        Ok(results)
    }
}

fn to_workspace_relative(path: &Path, root: &Path) -> Result<PathBuf, LocateError> {
    if !path.starts_with(root) {
        return Err(LocateError::OutsideWorkspaceRoot {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
        });
    }
    let rel = path.strip_prefix(root).unwrap();
    if rel.as_os_str().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        workspace_relative(rel).map_err(|_e| LocateError::OutsideWorkspaceRoot {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Spec: IgnoreWalkLocator must not traverse more than 32 directory levels
    /// deep. A Cargo.toml placed at level 33 must NOT be discovered.
    /// Without a max_depth cap, WalkBuilder traverses arbitrarily deep.
    #[test]
    fn ignore_walk_locator_does_not_traverse_beyond_32_levels() {
        let root = tempdir().unwrap();

        // Build a 33-level deep directory chain.
        let mut deep_dir = root.path().to_path_buf();
        for _ in 0..33 {
            deep_dir = deep_dir.join("sub");
        }
        fs::create_dir_all(&deep_dir).unwrap();

        // Place a valid Cargo.toml at the deepest level.
        fs::write(
            deep_dir.join("Cargo.toml"),
            "[package]\nname = \"deep-pkg\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let locator = IgnoreWalkLocator::new(root.path());
        let projects = locator.projects().unwrap();

        assert!(
            projects.is_empty(),
            "no projects should be found beyond 32 levels deep, found: {projects:?}"
        );
    }

    /// Spec: `IgnoreWalkLocator::discover` on a directory that has no workspace
    /// manifest markers (no Cargo.toml with [workspace], no package.json with
    /// workspaces field, no pnpm-workspace.yaml, no .moon directory) must
    /// return `Err(LocateError::WorkspaceRootNotFound)`, NOT a silent `Ok(None)`
    /// or a wrong-type error variant. This pins the error propagation path so
    /// that future refactors (e.g., adding a VCS probe to discover()) cannot
    /// accidentally swallow or mistype this error.
    #[test]
    fn discover_returns_workspace_root_not_found_for_non_workspace_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // Deliberately no workspace markers -- plain empty temp directory
        let result = IgnoreWalkLocator::discover(tmp.path());
        let is_correct = matches!(result, Err(LocateError::WorkspaceRootNotFound { .. }));
        let err_display = result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "<Ok(...)>".to_string());
        assert!(
            is_correct,
            "expected Err(LocateError::WorkspaceRootNotFound) for a directory with \
             no workspace manifest markers, got: {err_display}"
        );
    }

    /// Spec: when a directory contains both `Cargo.toml` (with a `[package]`
    /// section) and `package.json`, `projects()` must return both ecosystem
    /// entries AND sort Cargo before Npm -- Cargo ecosystem has explicit
    /// priority over Npm. This pins the sort order so that relying on enum
    /// discriminant ordering cannot silently break the precedence if the
    /// `Ecosystem` variant sequence is ever changed.
    #[test]
    fn projects_returns_cargo_before_npm_when_both_manifests_present() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"my-npm-pkg","version":"0.1.0"}"#,
        )
        .unwrap();

        let locator = IgnoreWalkLocator::new(root);
        let projects = locator.projects().unwrap();

        let cargo_pos = projects
            .iter()
            .position(|p| p.ecosystem == Ecosystem::Cargo);
        let npm_pos = projects.iter().position(|p| p.ecosystem == Ecosystem::Npm);

        assert!(
            cargo_pos.is_some(),
            "expected a Cargo project to be discovered in the results"
        );
        assert!(
            npm_pos.is_some(),
            "expected an Npm project to be discovered in the results"
        );
        assert!(
            cargo_pos.unwrap() < npm_pos.unwrap(),
            "Cargo must be sorted before Npm (explicit Cargo > npm precedence); \
             cargo_pos={:?}, npm_pos={:?}, projects={:?}",
            cargo_pos,
            npm_pos,
            projects
                .iter()
                .map(|p| format!("{:?}:{}", p.ecosystem, p.id.name()))
                .collect::<Vec<_>>()
        );
    }

    /// Spec (AC-01, AC-02): a Cargo.toml `[workspace]` `exclude` entry must
    /// prevent `projects()` from returning the excluded crate, while a crate
    /// that matches `members` and is not excluded must still be returned
    /// exactly once with `Ecosystem::Cargo`.
    #[test]
    fn ac01_ac02_excludes_scratch_example_and_includes_kept_example_via_cargo_workspace_exclude() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/scratch-example\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/scratch-example")).unwrap();
        std::fs::write(
            root.join("crates/scratch-example/Cargo.toml"),
            "[package]\nname = \"scratch-example\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/kept-example")).unwrap();
        std::fs::write(
            root.join("crates/kept-example/Cargo.toml"),
            "[package]\nname = \"kept-example\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects
                .iter()
                .any(|p| p.path == Path::new("crates/scratch-example")),
            "AC-01: crates/scratch-example must be excluded, got: {projects:?}"
        );
        let kept_count = projects
            .iter()
            .filter(|p| p.path == Path::new("crates/kept-example"))
            .count();
        assert_eq!(
            kept_count, 1,
            "AC-02: exactly one entry for crates/kept-example, got: {projects:?}"
        );
        let kept = projects
            .iter()
            .find(|p| p.path == Path::new("crates/kept-example"))
            .unwrap();
        assert_eq!(kept.ecosystem, Ecosystem::Cargo);
    }
}
