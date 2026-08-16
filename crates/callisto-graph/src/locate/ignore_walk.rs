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
        let npm_membership = membership::read_npm_membership(&self.root);
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
                            let is_root = rel == Path::new(".");
                            if npm_membership.admits(&rel, is_root) {
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

    /// Spec (AC-03): a root package.json declaring `{"workspaces": ["packages/*"]}`
    /// (no pnpm-workspace.yaml anywhere) must cause `projects()` to exclude a
    /// package.json found at a path outside every workspaces glob.
    #[test]
    fn ac03_excludes_package_outside_npm_workspaces_glob() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"workspaces": ["packages/*"]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("tools/helper")).unwrap();
        std::fs::write(
            root.join("tools/helper/package.json"),
            r#"{"name":"helper"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects.iter().any(|p| p.path == Path::new("tools/helper")),
            "AC-03: tools/helper must not be discovered, got: {projects:?}"
        );
    }

    /// Spec (AC-04): given a root with no package.json "workspaces" field but
    /// a sibling pnpm-workspace.yaml containing `packages:\n  - "packages/*"`,
    /// parsed via yaml_rust2::YamlLoader::load_from_str, a package.json at
    /// packages/kept/package.json (matching the glob) is discovered by
    /// projects(), and a package.json at tools/outside/package.json (not
    /// matching the glob) is not discovered by projects().
    #[test]
    fn ac04_pnpm_workspace_yaml_governs_npm_membership_when_no_workspaces_field() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/kept")).unwrap();
        std::fs::write(
            root.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(
            root.join("tools/outside/package.json"),
            r#"{"name":"outside"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/kept") && p.ecosystem == Ecosystem::Npm),
            "AC-04: packages/kept must be discovered as Npm, got: {projects:?}"
        );
        assert!(
            !projects
                .iter()
                .any(|p| p.path == Path::new("tools/outside")),
            "AC-04: tools/outside must not be discovered, got: {projects:?}"
        );
    }

    /// Spec (AC-05): given a Cargo.toml at the workspace root with no
    /// [workspace] table at all (a single-crate repo), every Cargo candidate
    /// directory discovered by the walk is included in projects() -- the
    /// absence of a [workspace] table means no membership filter applies,
    /// not that zero packages are admitted.
    #[test]
    fn ac05_admits_all_cargo_candidates_when_root_has_no_workspace_table() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/child")).unwrap();
        std::fs::write(
            root.join("crates/child/Cargo.toml"),
            "[package]\nname = \"child\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects.iter().any(|p| p.path == Path::new(".")),
            "root package must be admitted, got: {projects:?}"
        );
        assert!(
            projects.iter().any(|p| p.path == Path::new("crates/child")),
            "crates/child must be admitted, got: {projects:?}"
        );
    }

    /// Spec (AC-05b): given a workspace root with no Cargo.toml file at all
    /// (the file does not exist), and crates/kept/Cargo.toml elsewhere in
    /// the tree with a valid [package] table, the complete absence of a
    /// root Cargo.toml is treated identically to a root Cargo.toml with no
    /// [workspace] table (AC-05): no Cargo membership filter applies, not
    /// zero packages admitted, and the walk does not error.
    #[test]
    fn ac05b_admits_cargo_candidates_when_root_has_no_cargo_toml_file_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("crates/kept")).unwrap();
        std::fs::write(
            root.join("crates/kept/Cargo.toml"),
            "[package]\nname = \"kept\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("crates/kept") && p.ecosystem == Ecosystem::Cargo),
            "crates/kept must be admitted as Cargo, got: {projects:?}"
        );
    }

    /// AC-07: the root's [package]-declared crate is an implicit member
    /// exempt from the exclude list. Even a workspace root Cargo.toml whose
    /// exclude glob would textually match "." must still include the root
    /// package entry, and the members filter must still admit crates/child
    /// normally alongside the root exemption.
    #[test]
    fn ac07_includes_root_package_as_implicit_member_even_when_exclude_would_match_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"root-crate\"\nversion = \"0.1.0\"\n\n[workspace]\nmembers = [\"crates/*\"]\nexclude = [\".\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/child")).unwrap();
        std::fs::write(
            root.join("crates/child/Cargo.toml"),
            "[package]\nname = \"child\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        let root_entry = projects
            .iter()
            .find(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Cargo);
        assert!(
            root_entry.is_some(),
            "root package must never be silently dropped, got: {projects:?}"
        );
        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("crates/child") && p.ecosystem == Ecosystem::Cargo),
            "the members filter must still admit crates/child normally alongside the root exemption, got: {projects:?}"
        );
    }

    /// AC-10c: an explicitly empty `members = []` is a real filter matching
    /// nothing, not a no-op. A Cargo.toml elsewhere in the tree must be
    /// excluded from the Cargo ecosystem entries.
    #[test]
    fn ac10c_empty_members_array_admits_zero_cargo_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        std::fs::create_dir_all(root.join("crates/other")).unwrap();
        std::fs::write(
            root.join("crates/other/Cargo.toml"),
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects.iter().any(|p| p.ecosystem == Ecosystem::Cargo),
            "empty members = [] must exclude every Cargo candidate, got: {projects:?}"
        );
    }

    /// AC-15: a [workspace] table with an exclude key but no members key at
    /// all is treated identically to an absent [workspace] table for
    /// filtering purposes (every non-excluded candidate is admitted), not as
    /// an empty members = [] list which excludes everything (AC-10c).
    #[test]
    fn ac15_workspace_table_with_exclude_only_and_no_members_key_admits_non_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nexclude = [\"crates/foo\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/foo")).unwrap();
        std::fs::write(
            root.join("crates/foo/Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/kept")).unwrap();
        std::fs::write(
            root.join("crates/kept/Cargo.toml"),
            "[package]\nname = \"kept\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects.iter().any(|p| p.path == Path::new("crates/foo")),
            "excluded crate must not appear, got: {projects:?}"
        );
        assert!(
            projects.iter().any(|p| p.path == Path::new("crates/kept")),
            "non-excluded crate must appear, got: {projects:?}"
        );
    }

    /// Spec (AC-06): given a root package.json with no "workspaces" field
    /// and no pnpm-workspace.yaml anywhere in the workspace, every Npm
    /// candidate directory discovered by the walk is included in
    /// projects() -- absence of both markers means no npm membership
    /// filter applies. This mirrors AC-05's Cargo analog by asserting both
    /// the root package and the child package are admitted, not just one.
    #[test]
    fn ac06_admits_all_npm_candidates_when_no_workspaces_field_and_no_pnpm_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
        std::fs::create_dir_all(root.join("packages/child")).unwrap();
        std::fs::write(
            root.join("packages/child/package.json"),
            r#"{"name":"child"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "AC-06: root package must be admitted as Npm, got: {projects:?}"
        );
        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm),
            "AC-06: packages/child must be admitted as Npm, got: {projects:?}"
        );
    }

    /// Spec (AC-06b): given a workspace root directory containing no
    /// package.json file at all (the file does not exist at the root) and
    /// no pnpm-workspace.yaml anywhere in the workspace, and
    /// packages/kept/package.json elsewhere in the tree with a valid
    /// "name" field, projects() includes an entry with path ==
    /// "packages/kept" and ecosystem == Ecosystem::Npm -- the complete
    /// absence of a root package.json file is treated identically to a
    /// root package.json with no "workspaces" field (AC-06): no npm
    /// membership filter applies.
    #[test]
    fn ac06b_admits_npm_candidates_when_root_has_no_package_json_file_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("packages/kept")).unwrap();
        std::fs::write(
            root.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/kept") && p.ecosystem == Ecosystem::Npm),
            "AC-06b: packages/kept must be admitted as Npm, got: {projects:?}"
        );
    }

    /// Spec (AC-10): given a root package.json with "workspaces": []
    /// (present but empty array) and at least one package.json elsewhere in
    /// the tree, projects() returns zero Npm-ecosystem entries -- an
    /// explicitly empty workspaces list is a real membership filter
    /// (matches nothing), not a no-op.
    #[test]
    fn ac10_empty_npm_workspaces_array_admits_zero_npm_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"workspaces": []}"#).unwrap();
        std::fs::create_dir_all(root.join("packages/other")).unwrap();
        std::fs::write(
            root.join("packages/other/package.json"),
            r#"{"name":"other"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects.iter().any(|p| p.ecosystem == Ecosystem::Npm),
            "AC-10: no npm entries expected when workspaces = [], got: {projects:?}"
        );
    }

    /// Spec (AC-10, hybrid-root clause): the same empty "workspaces": []
    /// list must not exclude a hybrid root -- a root package.json that also
    /// declares a "name" field remains an implicit member of its own
    /// workspace (AC-16's exemption) and is admitted at path "." even
    /// though the empty workspaces list matches nothing.
    #[test]
    fn ac10_empty_npm_workspaces_array_still_admits_hybrid_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"name": "root-package", "workspaces": []}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/other")).unwrap();
        std::fs::write(
            root.join("packages/other/package.json"),
            r#"{"name":"other"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "AC-10: hybrid root at '.' must still be admitted, got: {projects:?}"
        );
        assert!(
            !projects
                .iter()
                .any(|p| p.path == Path::new("packages/other") && p.ecosystem == Ecosystem::Npm),
            "AC-10: non-root packages/other must remain excluded, got: {projects:?}"
        );
    }

    /// Spec (AC-10, distinguished-from-absent clause): an explicitly empty
    /// "workspaces": [] list excludes a sibling package.json, whereas a root
    /// package.json with no "workspaces" field at all (AC-06) admits the
    /// same sibling -- proving the empty list is a real filter distinct
    /// from, and not conflated with, an absent field.
    #[test]
    fn ac10_empty_workspaces_array_distinguished_from_absent_workspaces_field() {
        let tmp_empty = tempfile::tempdir().unwrap();
        let root_empty = tmp_empty.path();
        std::fs::write(root_empty.join("package.json"), r#"{"workspaces": []}"#).unwrap();
        std::fs::create_dir_all(root_empty.join("packages/other")).unwrap();
        std::fs::write(
            root_empty.join("packages/other/package.json"),
            r#"{"name":"other"}"#,
        )
        .unwrap();

        let tmp_absent = tempfile::tempdir().unwrap();
        let root_absent = tmp_absent.path();
        std::fs::write(root_absent.join("package.json"), r#"{"name":"root"}"#).unwrap();
        std::fs::create_dir_all(root_absent.join("packages/other")).unwrap();
        std::fs::write(
            root_absent.join("packages/other/package.json"),
            r#"{"name":"other"}"#,
        )
        .unwrap();

        let projects_empty = IgnoreWalkLocator::new(root_empty).projects().unwrap();
        let projects_absent = IgnoreWalkLocator::new(root_absent).projects().unwrap();

        assert!(
            !projects_empty
                .iter()
                .any(|p| p.path == Path::new("packages/other") && p.ecosystem == Ecosystem::Npm),
            "AC-10: empty workspaces = [] must exclude packages/other, got: {projects_empty:?}"
        );
        assert!(
            projects_absent
                .iter()
                .any(|p| p.path == Path::new("packages/other") && p.ecosystem == Ecosystem::Npm),
            "AC-06: absent workspaces field must admit packages/other, got: {projects_absent:?}"
        );
    }

    /// Spec (AC-08): given a root package.json declaring
    /// {"workspaces": ["packages/*"]} and a sibling pnpm-workspace.yaml
    /// declaring `packages:\n  - "tools/*"` (a different glob than the
    /// package.json field), projects() discovers a package.json at
    /// tools/x/package.json (matching only the pnpm-workspace.yaml glob)
    /// and does not discover a package.json at packages/y/package.json
    /// (matching only the package.json glob) -- pnpm-workspace.yaml
    /// governs npm membership filtering whenever both markers are present
    /// at the same root; the package.json "workspaces" field is ignored
    /// in that case.
    #[test]
    fn ac08_pnpm_workspace_yaml_takes_precedence_over_package_json_workspaces_field() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"workspaces": ["packages/*"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - \"tools/*\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("tools/x")).unwrap();
        std::fs::write(root.join("tools/x/package.json"), r#"{"name":"x"}"#).unwrap();
        std::fs::create_dir_all(root.join("packages/y")).unwrap();
        std::fs::write(root.join("packages/y/package.json"), r#"{"name":"y"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("tools/x") && p.ecosystem == Ecosystem::Npm),
            "AC-08: tools/x must be discovered as Npm, got: {projects:?}"
        );
        assert!(
            !projects.iter().any(|p| p.path == Path::new("packages/y")),
            "AC-08: packages/y must not be discovered, got: {projects:?}"
        );
    }

    /// AC-12: consolidated Cargo workspace-membership regression group.
    /// Reassembles the exact fixtures from AC-01/AC-02, AC-05, AC-05b,
    /// AC-07 (both the plain inclusion case and the exclude-exemption
    /// case), AC-10c, and AC-15 into a single regression test function per
    /// AC-12's literal "ships with at least one regression test using
    /// these exact fixtures" wording.
    #[test]
    fn ac12_cargo_workspace_membership_regression_group() {
        // AC-01 / AC-02
        {
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
            assert!(!projects
                .iter()
                .any(|p| p.path == Path::new("crates/scratch-example")));
            let kept_count = projects
                .iter()
                .filter(|p| p.path == Path::new("crates/kept-example"))
                .count();
            assert_eq!(kept_count, 1);
            let kept = projects
                .iter()
                .find(|p| p.path == Path::new("crates/kept-example"))
                .unwrap();
            assert_eq!(kept.ecosystem, Ecosystem::Cargo);
        }
        // AC-05
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects.iter().any(|p| p.path == Path::new(".")));
        }
        // AC-05b
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::create_dir_all(root.join("crates/kept")).unwrap();
            std::fs::write(
                root.join("crates/kept/Cargo.toml"),
                "[package]\nname = \"kept\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects.iter().any(|p| p.path == Path::new("crates/kept")));
        }
        // AC-07 (plain inclusion)
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"root-crate\"\nversion = \"0.1.0\"\n\n[workspace]\nmembers = [\"crates/*\"]\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Cargo));
        }
        // AC-07 (exclude-exemption: exclude = ["."] still admits the root)
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"root-crate\"\nversion = \"0.1.0\"\n\n[workspace]\nmembers = [\"crates/*\"]\nexclude = [\".\"]\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Cargo));
        }
        // AC-10c
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
            std::fs::create_dir_all(root.join("crates/other")).unwrap();
            std::fs::write(
                root.join("crates/other/Cargo.toml"),
                "[package]\nname = \"other\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(!projects.iter().any(|p| p.ecosystem == Ecosystem::Cargo));
        }
        // AC-15
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("Cargo.toml"),
                "[workspace]\nexclude = [\"crates/foo\"]\n",
            )
            .unwrap();
            std::fs::create_dir_all(root.join("crates/foo")).unwrap();
            std::fs::write(
                root.join("crates/foo/Cargo.toml"),
                "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            std::fs::create_dir_all(root.join("crates/kept")).unwrap();
            std::fs::write(
                root.join("crates/kept/Cargo.toml"),
                "[package]\nname = \"kept\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(!projects.iter().any(|p| p.path == Path::new("crates/foo")));
            assert!(projects.iter().any(|p| p.path == Path::new("crates/kept")));
        }
    }

    /// Spec (AC-10b): given a pnpm-workspace.yaml declaring `packages: []`
    /// (present, empty list) and at least one package.json elsewhere in the
    /// tree, projects() returns zero Npm-ecosystem entries besides any
    /// AC-17-style hybrid root package (a root package.json that also
    /// declares a "name" field is still an implicit member of its own
    /// workspace per AC-17's pnpm-governed exemption and is admitted at
    /// path "." even though the empty packages: list matches nothing) --
    /// mirrors AC-10 for the pnpm-driven case.
    #[test]
    fn ac10b_pnpm_empty_packages_list_admits_zero_npm_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
        std::fs::create_dir_all(root.join("packages/other")).unwrap();
        std::fs::write(
            root.join("packages/other/package.json"),
            r#"{"name":"other"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects.iter().any(|p| p.ecosystem == Ecosystem::Npm),
            "AC-10b: no non-root npm entries expected, got: {projects:?}"
        );
    }

    /// Spec (AC-10b, hybrid-root clause): the same empty `packages: []`
    /// list must not exclude a hybrid root -- a root package.json that
    /// also declares a "name" field remains an implicit member of its own
    /// workspace (AC-17's pnpm-governed exemption) and is admitted at
    /// path "." even though the empty packages: list matches nothing.
    #[test]
    fn ac10b_pnpm_empty_packages_list_still_admits_hybrid_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
        std::fs::write(root.join("package.json"), r#"{"name":"root-pkg"}"#).unwrap();
        std::fs::create_dir_all(root.join("packages/other")).unwrap();
        std::fs::write(
            root.join("packages/other/package.json"),
            r#"{"name":"other"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "AC-10b: hybrid root at '.' must still be admitted, got: {projects:?}"
        );
        assert!(
            !projects
                .iter()
                .any(|p| p.path == Path::new("packages/other") && p.ecosystem == Ecosystem::Npm),
            "AC-10b: non-root packages/other must remain excluded, got: {projects:?}"
        );
    }

    /// AC-11c: a sibling pnpm-workspace.yaml that fails to parse as YAML
    /// does not count as "present" for AC-08's precedence purposes -- the
    /// root package.json's "workspaces" field is still consulted and
    /// governs npm-ecosystem membership normally.
    #[test]
    fn ac11c_malformed_pnpm_yaml_does_not_count_as_present_for_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"workspaces": ["packages/*"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages: [\"packages/*\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/y")).unwrap();
        std::fs::write(root.join("packages/y/package.json"), r#"{"name":"y"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects.iter().any(|p| p.path == Path::new("packages/y")),
            "AC-11c: package.json workspaces field must still govern membership when \
             pnpm-workspace.yaml is malformed YAML, got: {projects:?}"
        );
    }

    /// AC-11: a root package.json with no "workspaces" field, alongside a
    /// pnpm-workspace.yaml whose YAML is malformed (unterminated flow
    /// sequence -> yaml_rust2::YamlLoader::load_from_str returns Err), must
    /// not panic or error the whole walk. The npm membership filter is
    /// treated as absent for that workspace (falls back to AC-06's
    /// admit-all behavior), matching AC-09's malformed-TOML treatment.
    #[test]
    fn ac11_malformed_pnpm_yaml_with_no_package_json_workspaces_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages: [\"packages/*\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/kept")).unwrap();
        std::fs::write(
            root.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/kept")),
            "AC-11: malformed pnpm-workspace.yaml with no package.json workspaces field \
             must fall back to admit-all npm membership, got: {projects:?}"
        );
    }

    /// AC-13: consolidated npm/pnpm workspace-membership regression group.
    /// Reassembles the exact fixtures required by AC-03, AC-04, AC-06,
    /// AC-06b, AC-08, AC-10, AC-10b, AC-10d, AC-11c, AC-16, AC-16b, and
    /// AC-17 into a single regression test.
    #[test]
    fn ac13_npm_pnpm_workspace_membership_regression_group() {
        // AC-03
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("package.json"),
                r#"{"workspaces": ["packages/*"]}"#,
            )
            .unwrap();
            std::fs::create_dir_all(root.join("tools/helper")).unwrap();
            std::fs::write(
                root.join("tools/helper/package.json"),
                r#"{"name":"helper"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(!projects.iter().any(|p| p.path == Path::new("tools/helper")));
        }
        // AC-04
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("pnpm-workspace.yaml"),
                "packages:\n  - \"packages/*\"\n",
            )
            .unwrap();
            std::fs::create_dir_all(root.join("packages/kept")).unwrap();
            std::fs::write(
                root.join("packages/kept/package.json"),
                r#"{"name":"kept"}"#,
            )
            .unwrap();
            std::fs::create_dir_all(root.join("tools/outside")).unwrap();
            std::fs::write(
                root.join("tools/outside/package.json"),
                r#"{"name":"outside"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/kept")));
            assert!(!projects
                .iter()
                .any(|p| p.path == Path::new("tools/outside")));
        }
        // AC-06
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
            std::fs::create_dir_all(root.join("packages/child")).unwrap();
            std::fs::write(
                root.join("packages/child/package.json"),
                r#"{"name":"child"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm));
        }
        // AC-06b
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::create_dir_all(root.join("packages/kept")).unwrap();
            std::fs::write(
                root.join("packages/kept/package.json"),
                r#"{"name":"kept"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/kept")));
        }
        // AC-08
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("package.json"),
                r#"{"workspaces": ["packages/*"]}"#,
            )
            .unwrap();
            std::fs::write(
                root.join("pnpm-workspace.yaml"),
                "packages:\n  - \"tools/*\"\n",
            )
            .unwrap();
            std::fs::create_dir_all(root.join("tools/x")).unwrap();
            std::fs::write(root.join("tools/x/package.json"), r#"{"name":"x"}"#).unwrap();
            std::fs::create_dir_all(root.join("packages/y")).unwrap();
            std::fs::write(root.join("packages/y/package.json"), r#"{"name":"y"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects.iter().any(|p| p.path == Path::new("tools/x")));
            assert!(!projects.iter().any(|p| p.path == Path::new("packages/y")));
        }
        // AC-10b
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
            std::fs::create_dir_all(root.join("packages/other")).unwrap();
            std::fs::write(
                root.join("packages/other/package.json"),
                r#"{"name":"other"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(!projects.iter().any(|p| p.ecosystem == Ecosystem::Npm));
        }
        // AC-11c
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("package.json"),
                r#"{"workspaces": ["packages/*"]}"#,
            )
            .unwrap();
            std::fs::write(
                root.join("pnpm-workspace.yaml"),
                "packages: [\"packages/*\"\n",
            )
            .unwrap();
            std::fs::create_dir_all(root.join("packages/y")).unwrap();
            std::fs::write(root.join("packages/y/package.json"), r#"{"name":"y"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects.iter().any(|p| p.path == Path::new("packages/y")));
        }

        // AC-10
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("package.json"), r#"{"workspaces": []}"#).unwrap();
            std::fs::create_dir_all(root.join("packages/other")).unwrap();
            std::fs::write(
                root.join("packages/other/package.json"),
                r#"{"name":"other"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(!projects.iter().any(|p| p.ecosystem == Ecosystem::Npm));
        }
        // AC-16
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("package.json"),
                r#"{"name": "root-package", "workspaces": ["packages/*"]}"#,
            )
            .unwrap();
            std::fs::create_dir_all(root.join("packages/child")).unwrap();
            std::fs::write(
                root.join("packages/child/package.json"),
                r#"{"name":"child"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm));
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm));
        }
        // AC-16b
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("package.json"),
                r#"{"name": "root-package", "workspaces": ["packages/*"]}"#,
            )
            .unwrap();
            std::fs::write(
                root.join("pnpm-workspace.yaml"),
                "packages: [\"packages/*\"\n",
            )
            .unwrap();
            std::fs::create_dir_all(root.join("packages/child")).unwrap();
            std::fs::write(
                root.join("packages/child/package.json"),
                r#"{"name":"child"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm));
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm));
        }
        // AC-10d
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("package.json"),
                r#"{"name": "root-package", "workspaces": []}"#,
            )
            .unwrap();
            std::fs::create_dir_all(root.join("packages/other")).unwrap();
            std::fs::write(
                root.join("packages/other/package.json"),
                r#"{"name":"other"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            let npm_entries: Vec<_> = projects
                .iter()
                .filter(|p| p.ecosystem == Ecosystem::Npm)
                .collect();
            assert_eq!(npm_entries.len(), 1);
            assert_eq!(npm_entries[0].path, Path::new("."));
        }
        // AC-17
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("package.json"), r#"{"name": "root-package"}"#).unwrap();
            std::fs::write(
                root.join("pnpm-workspace.yaml"),
                "packages:\n  - \"packages/*\"\n",
            )
            .unwrap();
            std::fs::create_dir_all(root.join("packages/child")).unwrap();
            std::fs::write(
                root.join("packages/child/package.json"),
                r#"{"name":"child"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm));
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm));
        }
    }

    #[test]
    fn ac09_malformed_cargo_members_bare_string_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = \"crates/*\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(
            root.join("tools/outside/Cargo.toml"),
            "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn ac09_malformed_cargo_members_non_string_entry_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\", 42]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(
            root.join("tools/outside/Cargo.toml"),
            "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn ac09d_malformed_root_cargo_toml_syntax_falls_back_to_absent_cargo_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace\nmembers = [\n").unwrap();
        std::fs::create_dir_all(root.join("crates/kept")).unwrap();
        std::fs::write(
            root.join("crates/kept/Cargo.toml"),
            "[package]\nname = \"kept\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects.iter().any(|p| p.path == Path::new("crates/kept")));
    }

    #[test]
    fn ac09b_malformed_cargo_exclude_bare_string_falls_back_to_excluding_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = \"crates/scratch-example\"\n",
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
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(
            root.join("tools/outside/Cargo.toml"),
            "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("crates/scratch-example")));
        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("crates/kept-example")));
        assert!(!projects
            .iter()
            .any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn ac09b_malformed_cargo_exclude_non_string_entry_falls_back_to_excluding_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/scratch-example\", 42]\n",
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
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(
            root.join("tools/outside/Cargo.toml"),
            "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("crates/scratch-example")));
        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("crates/kept-example")));
        assert!(!projects
            .iter()
            .any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn ac09f_invalid_glob_in_cargo_members_is_skipped_without_disabling_sibling_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/kept-example\", \"crates/[unterminated\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/kept-example")).unwrap();
        std::fs::write(
            root.join("crates/kept-example/Cargo.toml"),
            "[package]\nname = \"kept-example\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("crates/kept-example")));
    }

    #[test]
    fn ac09g_invalid_glob_in_npm_workspaces_is_skipped_without_disabling_sibling_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"workspaces": ["packages/kept-example", "packages/[unterminated"]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/kept-example")).unwrap();
        std::fs::write(
            root.join("packages/kept-example/package.json"),
            r#"{"name":"kept-example"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("packages/kept-example")));
    }

    #[test]
    fn ac09i_invalid_glob_in_cargo_exclude_is_skipped_without_disabling_sibling_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/scratch-example\", \"crates/[unterminated\"]\n",
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

        assert!(!projects
            .iter()
            .any(|p| p.path == Path::new("crates/scratch-example")));
        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("crates/kept-example")));
    }

    #[test]
    fn ac09h_invalid_glob_in_pnpm_packages_is_skipped_without_disabling_sibling_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/kept-example\"\n  - \"packages/[unterminated\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/kept-example")).unwrap();
        std::fs::write(
            root.join("packages/kept-example/package.json"),
            r#"{"name":"kept-example"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("packages/kept-example")));
    }

    #[test]
    fn ac09c_malformed_npm_workspaces_bare_string_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"workspaces": "packages/*"}"#).unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(
            root.join("tools/outside/package.json"),
            r#"{"name":"outside"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn ac09c_malformed_npm_workspaces_non_string_entry_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"workspaces": ["packages/*", 42]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(
            root.join("tools/outside/package.json"),
            r#"{"name":"outside"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn ac09e_malformed_root_package_json_syntax_falls_back_to_absent_npm_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            "{\"workspaces\": [\"packages/*\"],}",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/kept")).unwrap();
        std::fs::write(
            root.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects
            .iter()
            .any(|p| p.path == Path::new("packages/kept")));
    }

    /// Spec (AC-11b): a pnpm-workspace.yaml that parses successfully but
    /// whose top-level document either has no `packages:` key, or has a
    /// `packages:` key whose value is not a YAML sequence of strings --
    /// e.g. `packages: "packages/*"` (bare scalar), `packages: {foo: bar}`
    /// (mapping), or a sequence containing a non-string element -- does not
    /// panic; `IgnoreWalkLocator::projects()` treats the npm membership
    /// filter as absent for that workspace, the same fallback as AC-11's
    /// YAML-syntax-error case.
    #[test]
    fn ac11b_pnpm_packages_wrong_shape_falls_back_to_absent_filter() {
        let dir_missing = tempfile::tempdir().unwrap();
        let root_missing = dir_missing.path();
        std::fs::write(
            root_missing.join("pnpm-workspace.yaml"),
            "other_key: true\n",
        )
        .unwrap();
        std::fs::create_dir_all(root_missing.join("packages/kept")).unwrap();
        std::fs::write(
            root_missing.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();
        let projects_missing = IgnoreWalkLocator::new(root_missing).projects().unwrap();
        assert!(
            projects_missing
                .iter()
                .any(|p| p.path == Path::new("packages/kept")),
            "AC-11b (missing key): packages/kept must be admitted, got: {projects_missing:?}"
        );

        let dir_scalar = tempfile::tempdir().unwrap();
        let root_scalar = dir_scalar.path();
        std::fs::write(
            root_scalar.join("pnpm-workspace.yaml"),
            "packages: \"packages/*\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root_scalar.join("packages/kept")).unwrap();
        std::fs::write(
            root_scalar.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();
        let projects_scalar = IgnoreWalkLocator::new(root_scalar).projects().unwrap();
        assert!(
            projects_scalar
                .iter()
                .any(|p| p.path == Path::new("packages/kept")),
            "AC-11b (wrong scalar type): packages/kept must be admitted, got: {projects_scalar:?}"
        );

        let dir_mapping = tempfile::tempdir().unwrap();
        let root_mapping = dir_mapping.path();
        std::fs::write(
            root_mapping.join("pnpm-workspace.yaml"),
            "packages:\n  foo: bar\n",
        )
        .unwrap();
        std::fs::create_dir_all(root_mapping.join("packages/kept")).unwrap();
        std::fs::write(
            root_mapping.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();
        let projects_mapping = IgnoreWalkLocator::new(root_mapping).projects().unwrap();
        assert!(
            projects_mapping
                .iter()
                .any(|p| p.path == Path::new("packages/kept")),
            "AC-11b (mapping value): packages/kept must be admitted, got: {projects_mapping:?}"
        );

        let dir_nonstring = tempfile::tempdir().unwrap();
        let root_nonstring = dir_nonstring.path();
        std::fs::write(
            root_nonstring.join("pnpm-workspace.yaml"),
            "packages:\n  - \"a\"\n  - 42\n",
        )
        .unwrap();
        std::fs::create_dir_all(root_nonstring.join("packages/kept")).unwrap();
        std::fs::write(
            root_nonstring.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();
        let projects_nonstring = IgnoreWalkLocator::new(root_nonstring).projects().unwrap();
        assert!(
            projects_nonstring
                .iter()
                .any(|p| p.path == Path::new("packages/kept")),
            "AC-11b (non-string sequence element): packages/kept must be admitted, got: {projects_nonstring:?}"
        );
    }

    /// Spec (AC-11d): a pnpm-workspace.yaml file that is empty (zero bytes),
    /// contains only whitespace, or contains only YAML comments -- such that
    /// yaml_rust2::YamlLoader::load_from_str returns Ok(vec![]), a
    /// successful parse producing zero YAML documents rather than a single
    /// document -- does not panic (IgnoreWalkLocator::projects() must not
    /// unconditionally index docs[0] or call docs.first().unwrap()); it
    /// treats the npm membership filter as absent for that workspace, the
    /// same fallback as AC-11b's missing-packages-key case.
    #[test]
    fn ac11d_zero_yaml_documents_falls_back_to_absent_filter_without_panic() {
        let dir_empty = tempfile::tempdir().unwrap();
        let root_empty = dir_empty.path();
        std::fs::write(root_empty.join("pnpm-workspace.yaml"), "").unwrap();
        std::fs::create_dir_all(root_empty.join("packages/kept")).unwrap();
        std::fs::write(
            root_empty.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();
        let projects_empty = IgnoreWalkLocator::new(root_empty).projects().unwrap();
        assert!(
            projects_empty
                .iter()
                .any(|p| p.path == Path::new("packages/kept")),
            "AC-11d (zero bytes): packages/kept must be admitted, got: {projects_empty:?}"
        );

        let dir_whitespace = tempfile::tempdir().unwrap();
        let root_whitespace = dir_whitespace.path();
        std::fs::write(root_whitespace.join("pnpm-workspace.yaml"), "   \n\t\n  \n").unwrap();
        std::fs::create_dir_all(root_whitespace.join("packages/kept")).unwrap();
        std::fs::write(
            root_whitespace.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();
        let projects_whitespace = IgnoreWalkLocator::new(root_whitespace).projects().unwrap();
        assert!(
            projects_whitespace
                .iter()
                .any(|p| p.path == Path::new("packages/kept")),
            "AC-11d (whitespace only): packages/kept must be admitted, got: {projects_whitespace:?}"
        );

        let dir_comments = tempfile::tempdir().unwrap();
        let root_comments = dir_comments.path();
        std::fs::write(
            root_comments.join("pnpm-workspace.yaml"),
            "# no packages here\n# another comment\n",
        )
        .unwrap();
        std::fs::create_dir_all(root_comments.join("packages/kept")).unwrap();
        std::fs::write(
            root_comments.join("packages/kept/package.json"),
            r#"{"name":"kept"}"#,
        )
        .unwrap();
        let projects_comments = IgnoreWalkLocator::new(root_comments).projects().unwrap();
        assert!(
            projects_comments
                .iter()
                .any(|p| p.path == Path::new("packages/kept")),
            "AC-11d (comments only): packages/kept must be admitted, got: {projects_comments:?}"
        );
    }

    /// AC-14 (Cargo slice): regression-proof consolidation of the Cargo
    /// malformed-input criteria already proven individually as
    /// ac09_malformed_cargo_members_bare_string_falls_back_to_absent_filter,
    /// ac09b_malformed_cargo_exclude_bare_string_falls_back_to_excluding_nothing,
    /// ac09d_malformed_root_cargo_toml_syntax_falls_back_to_absent_cargo_filter,
    /// ac09f_invalid_glob_in_cargo_members_is_skipped_without_disabling_sibling_entries,
    /// and ac09i_invalid_glob_in_cargo_exclude_is_skipped_without_disabling_sibling_entries.
    /// Reassembles the exact fixtures from AC-14's spec text for the Cargo
    /// sub-slice (AC-09, AC-09b, AC-09d, AC-09f, AC-09i) into a single
    /// regression group so future changes to Cargo membership handling that
    /// break any one of these no-panic/fallback behaviors are caught here.
    #[test]
    fn ac14_cargo_malformed_edge_case_regression_slice() {
        // AC-09: bare-string `members` falls back to absent filter (admits everything).
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(
                root.join("Cargo.toml"),
                "[workspace]\nmembers = \"crates/*\"\n",
            )
            .unwrap();
            fs::create_dir_all(root.join("tools/outside")).unwrap();
            fs::write(
                root.join("tools/outside/Cargo.toml"),
                "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects
                    .iter()
                    .any(|p| p.path == Path::new("tools/outside")),
                "AC-09: tools/outside must be admitted, got: {projects:?}"
            );
        }

        // AC-09b: bare-string `exclude` falls back to excluding nothing, while
        // the well-formed `members` filter still applies.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(
                root.join("Cargo.toml"),
                "[workspace]\nmembers = [\"crates/*\"]\nexclude = \"crates/scratch-example\"\n",
            )
            .unwrap();
            fs::create_dir_all(root.join("crates/scratch-example")).unwrap();
            fs::write(
                root.join("crates/scratch-example/Cargo.toml"),
                "[package]\nname = \"scratch-example\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            fs::create_dir_all(root.join("crates/kept-example")).unwrap();
            fs::write(
                root.join("crates/kept-example/Cargo.toml"),
                "[package]\nname = \"kept-example\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            fs::create_dir_all(root.join("tools/outside")).unwrap();
            fs::write(
                root.join("tools/outside/Cargo.toml"),
                "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects
                    .iter()
                    .any(|p| p.path == Path::new("crates/scratch-example")),
                "AC-09b: crates/scratch-example must be admitted, got: {projects:?}"
            );
            assert!(
                projects
                    .iter()
                    .any(|p| p.path == Path::new("crates/kept-example")),
                "AC-09b: crates/kept-example must be admitted, got: {projects:?}"
            );
            assert!(
                !projects
                    .iter()
                    .any(|p| p.path == Path::new("tools/outside")),
                "AC-09b: tools/outside must remain excluded by members, got: {projects:?}"
            );
        }

        // AC-09d: syntactically invalid root TOML falls back to absent Cargo filter.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("Cargo.toml"), "[workspace\nmembers = [\n").unwrap();
            fs::create_dir_all(root.join("crates/kept")).unwrap();
            fs::write(
                root.join("crates/kept/Cargo.toml"),
                "[package]\nname = \"kept\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects.iter().any(|p| p.path == Path::new("crates/kept")),
                "AC-09d: crates/kept must be admitted, got: {projects:?}"
            );
        }

        // AC-09f: an invalid glob entry in `members` is skipped without
        // disabling sibling well-formed entries.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(
                root.join("Cargo.toml"),
                "[workspace]\nmembers = [\"crates/kept-example\", \"crates/[unterminated\"]\n",
            )
            .unwrap();
            fs::create_dir_all(root.join("crates/kept-example")).unwrap();
            fs::write(
                root.join("crates/kept-example/Cargo.toml"),
                "[package]\nname = \"kept-example\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects
                    .iter()
                    .any(|p| p.path == Path::new("crates/kept-example")),
                "AC-09f: crates/kept-example must be admitted, got: {projects:?}"
            );
        }

        // AC-09i: an invalid glob entry in `exclude` is skipped without
        // disabling sibling well-formed exclude entries.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(
                root.join("Cargo.toml"),
                "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/scratch-example\", \"crates/[unterminated\"]\n",
            )
            .unwrap();
            fs::create_dir_all(root.join("crates/scratch-example")).unwrap();
            fs::write(
                root.join("crates/scratch-example/Cargo.toml"),
                "[package]\nname = \"scratch-example\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            fs::create_dir_all(root.join("crates/kept-example")).unwrap();
            fs::write(
                root.join("crates/kept-example/Cargo.toml"),
                "[package]\nname = \"kept-example\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                !projects
                    .iter()
                    .any(|p| p.path == Path::new("crates/scratch-example")),
                "AC-09i: crates/scratch-example must remain excluded, got: {projects:?}"
            );
            assert!(
                projects
                    .iter()
                    .any(|p| p.path == Path::new("crates/kept-example")),
                "AC-09i: crates/kept-example must be admitted, got: {projects:?}"
            );
        }
    }

    /// AC-14 (npm slice): consolidated regression group reassembling the
    /// fixtures already proven individually by
    /// ac09c_malformed_workspaces_field_falls_back_to_absent_npm_filter,
    /// ac09e_malformed_root_package_json_syntax_falls_back_to_absent_npm_filter,
    /// ac09g_invalid_glob_in_npm_workspaces_is_skipped_without_disabling_sibling_entries,
    /// and ac10_empty_npm_workspaces_array_excludes_everything. Reassembles
    /// the exact fixtures from AC-14's spec text for the npm sub-slice
    /// (AC-09c, AC-09e, AC-09g, AC-10) into a single regression group so
    /// future changes to npm membership handling that break any one of
    /// these no-panic/fallback behaviors are caught here.
    #[test]
    fn ac14_npm_malformed_edge_case_regression_slice() {
        // AC-09c: bare-string `workspaces` falls back to absent npm filter
        // (admits everything, including paths outside any glob).
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("package.json"), r#"{"workspaces": "packages/*"}"#).unwrap();
            fs::create_dir_all(root.join("tools/outside")).unwrap();
            fs::write(
                root.join("tools/outside/package.json"),
                r#"{"name":"outside"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects
                    .iter()
                    .any(|p| p.path == Path::new("tools/outside")),
                "AC-09c: tools/outside must be admitted, got: {projects:?}"
            );
        }

        // AC-09e: syntactically invalid JSON falls back to absent npm
        // filter without panicking or erroring the whole walk.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(
                root.join("package.json"),
                "{\"workspaces\": [\"packages/*\"],}",
            )
            .unwrap();
            fs::create_dir_all(root.join("packages/kept")).unwrap();
            fs::write(
                root.join("packages/kept/package.json"),
                r#"{"name":"kept"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects
                    .iter()
                    .any(|p| p.path == Path::new("packages/kept")),
                "AC-09e: packages/kept must be admitted, got: {projects:?}"
            );
        }

        // AC-09g: an invalid glob entry in npm `workspaces` is skipped
        // without disabling sibling well-formed entries.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(
                root.join("package.json"),
                r#"{"workspaces": ["packages/kept-example", "packages/[unterminated"]}"#,
            )
            .unwrap();
            fs::create_dir_all(root.join("packages/kept-example")).unwrap();
            fs::write(
                root.join("packages/kept-example/package.json"),
                r#"{"name":"kept-example"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects
                    .iter()
                    .any(|p| p.path == Path::new("packages/kept-example")),
                "AC-09g: packages/kept-example must be admitted, got: {projects:?}"
            );
        }

        // AC-10: an explicitly empty `workspaces` array is a real
        // membership filter that matches nothing (distinguished from an
        // absent `workspaces` field).
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("package.json"), r#"{"workspaces": []}"#).unwrap();
            fs::create_dir_all(root.join("packages/other")).unwrap();
            fs::write(
                root.join("packages/other/package.json"),
                r#"{"name":"other"}"#,
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                !projects.iter().any(|p| p.ecosystem == Ecosystem::Npm),
                "AC-10: no Npm entries expected, got: {projects:?}"
            );
        }
    }
}
