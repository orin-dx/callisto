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
        IgnoreWalkLocator { root: canonical, skip }
    }

    pub fn discover(start: &Path, runner: &dyn callisto_model::CommandRunner) -> Result<Self, LocateError> {
        let root = find_workspace_root(start, runner)?;
        Ok(Self::new(&root))
    }
}

impl ProjectLocator for IgnoreWalkLocator {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError> {
        Ok(self.projects_and_platform_candidates()?.0)
    }

    fn projects_and_platform_candidates(&self) -> Result<(Vec<ProjectRoot>, Vec<ProjectRoot>), LocateError> {
        let mut results = Vec::new();
        let mut platform_candidates = Vec::new();
        let cargo_membership = membership::read_cargo_membership(&self.root);
        let npm_membership = membership::read_npm_membership(&self.root);
        let python_membership = membership::read_python_membership(&self.root);
        let walker = WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .parents(false)
            .max_depth(Some(32))
            // Symlinked package directories (e.g. vendor-link style monorepos) are a
            // supported workspace pattern -- discovery follows them deliberately. The
            // existing max_depth(32) cap already bounds a symlink cycle from hanging.
            .follow_links(true)
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

            let rel = to_workspace_relative(path, &self.root)?;
            let is_root = rel == Path::new(".");
            let admits = |ecosystem: Ecosystem| match ecosystem {
                Ecosystem::Cargo => cargo_membership.admits(&rel, is_root),
                Ecosystem::Npm => npm_membership.admits(&rel, is_root),
                Ecosystem::Pypi => python_membership.admits(&rel, is_root),
                _ => false,
            };
            // Own-path check for a *failed* manifest: only a real, explicit members/workspaces
            // match counts -- the admit-all fallback (no restriction present, e.g. because this
            // very manifest is the one that failed to parse) must not count as membership.
            let admits_explicitly = |ecosystem: Ecosystem| match ecosystem {
                Ecosystem::Cargo => cargo_membership.admits_explicitly(&rel, is_root),
                Ecosystem::Npm => npm_membership.admits_explicitly(&rel, is_root),
                Ecosystem::Pypi => python_membership.admits_explicitly(&rel, is_root),
                _ => false,
            };

            // First pass: read and parse every canonical manifest present in this
            // directory, so a parse failure below can check whether a *sibling*
            // manifest here was admitted, not only its own ecosystem's membership.
            let mut parsed: Vec<(Ecosystem, String, callisto_manifests::ManifestIdentity)> = Vec::new();
            let mut failures: Vec<(PathBuf, String)> = Vec::new();
            for ecosystem in Ecosystem::CANONICAL {
                // Every `Ecosystem::CANONICAL` member has a canonical
                // manifest format by construction -- see its doc comment.
                let format = ecosystem
                    .canonical_manifest_format()
                    .expect("CANONICAL ecosystems always have a canonical manifest format");
                let manifest_path = path.join(format.file_name());
                let Ok(content) = fs::read_to_string(&manifest_path) else {
                    continue;
                };
                match callisto_manifests::read_identity(format, &content, &manifest_path) {
                    Ok(identity) => parsed.push((ecosystem, content, identity)),
                    Err(e) => failures.push((manifest_path, e.to_string())),
                }
            }

            // Explicit only, like `self_admitted` below -- a sibling that's merely admitted by
            // the fallback admit-all (no restriction present for that ecosystem) doesn't make
            // this directory a real, deliberate workspace member; only a genuine members/
            // workspaces match does.
            let any_sibling_admitted = parsed
                .iter()
                .any(|(ecosystem, _, identity)| identity.name.is_some() && admits_explicitly(*ecosystem));

            for (manifest_path, message) in failures {
                // Determine which ecosystem this failed manifest belongs to from its
                // file name, to check its own membership independent of any sibling.
                let ecosystem = Ecosystem::CANONICAL.into_iter().find(|eco| {
                    eco.canonical_manifest_format()
                        .is_some_and(|f| Some(f.file_name()) == manifest_path.file_name().and_then(|n| n.to_str()))
                });
                let self_admitted = ecosystem.is_some_and(admits_explicitly);
                if self_admitted || any_sibling_admitted {
                    return Err(LocateError::ManifestParseError {
                        path: manifest_path,
                        message,
                    });
                }
                eprintln!("{}", unparseable_manifest_warning(&manifest_path, &message));
            }

            for (ecosystem, content, identity) in parsed {
                let Some(name) = identity.name else {
                    continue;
                };

                // A private, versionless root package.json (standard npm/pnpm workspace-root
                // layout, e.g. `@changesets/cli`) declares a name only to hold `workspaces`; it
                // is never itself a released package, so it never becomes a project.
                if is_root
                    && ecosystem == Ecosystem::Npm
                    && identity.version.is_none()
                    && callisto_manifests::npm_declares_private(&content)
                {
                    continue;
                }

                let admitted = admits(ecosystem);
                let id = PackageId::parse(&name).unwrap_or_else(|_| PackageId::Bare(name.clone()));
                let project = ProjectRoot {
                    id,
                    path: rel.clone(),
                    ecosystem,
                };
                if admitted {
                    results.push(project);
                } else if ecosystem == Ecosystem::Npm && callisto_manifests::npm_role_from_source(&content).is_some() {
                    platform_candidates.push(project);
                }
            }
        }

        results.sort_by(|a, b| (&a.path, a.ecosystem).cmp(&(&b.path, b.ecosystem)));
        platform_candidates.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((results, platform_candidates))
    }
}

/// Warning text for a manifest that fails to parse but isn't a workspace member (its own
/// ecosystem doesn't admit it, and it shares no directory with an admitted manifest) --
/// tested directly, since `eprintln!` output can't be captured from inside discovery.
fn unparseable_manifest_warning(path: &Path, message: &str) -> String {
    format!("warning: failed to parse manifest `{}`: {message}", path.display())
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

    /// Spec: symlinked directories are a deliberately supported workspace
    /// pattern (vendor-link style monorepos). `IgnoreWalkLocator` must
    /// traverse *into* a symlinked directory and discover a package nested
    /// below its top level, not just a manifest sitting directly at the
    /// symlink's root -- proving `follow_links(true)` is really wired in,
    /// not just tolerated by an incidental `is_dir()` resolution.
    #[test]
    fn ignore_walk_locator_discovers_package_nested_inside_a_symlinked_directory() {
        let root = tempdir().unwrap();
        let external = tempdir().unwrap();

        let nested = external.path().join("nested-pkg");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            nested.join("Cargo.toml"),
            "[package]\nname = \"linked-nested-pkg\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        std::os::unix::fs::symlink(external.path(), root.path().join("vendor-link"))
            .expect("failed to create symlink for test fixture");

        let locator = IgnoreWalkLocator::new(root.path());
        let projects = locator.projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.id == PackageId::Bare("linked-nested-pkg".to_string())),
            "expected linked-nested-pkg discovered through the symlinked directory, got: {projects:?}"
        );
    }

    /// Spec: a `pyproject.toml` workspace's `[tool.uv.workspace] exclude`
    /// entry must be honored by discovery, matching the Cargo/npm branches'
    /// own membership filtering -- a Python package under an excluded path
    /// must not be admitted as a release-managed package.
    #[test]
    fn ignore_walk_locator_honors_python_workspace_exclude() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"root-pkg\"\nversion = \"0.1.0\"\n\n[tool.uv.workspace]\nmembers = [\"packages/*\"]\nexclude = [\"packages/examples/demo\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("packages/examples/demo")).unwrap();
        fs::write(
            root.join("packages/examples/demo/pyproject.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("packages/kept")).unwrap();
        fs::write(
            root.join("packages/kept/pyproject.toml"),
            "[project]\nname = \"kept\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let locator = IgnoreWalkLocator::new(root);
        let projects = locator.projects().unwrap();

        assert!(
            !projects.iter().any(|p| p.path == Path::new("packages/examples/demo")),
            "excluded Python package must not be discovered, got: {projects:?}"
        );
        assert!(
            projects.iter().any(|p| p.path == Path::new("packages/kept")),
            "non-excluded Python package must still be discovered, got: {projects:?}"
        );
    }

    /// Regression: before routing name extraction through the shared
    /// `callisto_manifests::read_identity` (which falls back through PEP
    /// 621 -> Poetry -> Flit, matching `python_package_name`), this walker's
    /// own hand-rolled pyproject.toml parsing only checked `project.name`
    /// and `tool.poetry.name`, silently failing to discover a Flit-based
    /// Python package. Closing this gap is a side effect of removing the
    /// duplication (audit pattern B).
    #[test]
    fn discovers_flit_based_python_package_via_shared_identity_reader() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("pyproject.toml"),
            "[tool.flit.metadata]\nmodule = \"my_flit_lib\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.id == PackageId::Bare("my_flit_lib".to_string()) && p.ecosystem == Ecosystem::Pypi),
            "Flit-based package must be discovered via [tool.flit.metadata].module, got: {projects:?}"
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
        struct FakeGitToplevel;
        impl callisto_model::CommandRunner for FakeGitToplevel {
            fn run(
                &self,
                _program: &str,
                _args: &[&str],
                cwd: &std::path::Path,
            ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
                Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout: format!("{}\n", cwd.display()),
                    stderr: String::new(),
                })
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        // A Git repository with deliberately no workspace or package markers.
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        let result = IgnoreWalkLocator::discover(tmp.path(), &FakeGitToplevel);
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
        std::fs::write(root.join("package.json"), r#"{"name":"my-npm-pkg","version":"0.1.0"}"#).unwrap();

        let locator = IgnoreWalkLocator::new(root);
        let projects = locator.projects().unwrap();

        let cargo_pos = projects.iter().position(|p| p.ecosystem == Ecosystem::Cargo);
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

    /// A Cargo.toml `[workspace]` `exclude` entry must
    /// prevent `projects()` from returning the excluded crate, while a crate
    /// that matches `members` and is not excluded must still be returned
    /// exactly once with `Ecosystem::Cargo`.
    #[test]
    fn excludes_scratch_example_and_includes_kept_example_via_cargo_workspace_exclude() {
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
            !projects.iter().any(|p| p.path == Path::new("crates/scratch-example")),
            "crates/scratch-example must be excluded, got: {projects:?}"
        );
        let kept_count = projects
            .iter()
            .filter(|p| p.path == Path::new("crates/kept-example"))
            .count();
        assert_eq!(
            kept_count, 1,
            "exactly one entry for crates/kept-example, got: {projects:?}"
        );
        let kept = projects
            .iter()
            .find(|p| p.path == Path::new("crates/kept-example"))
            .unwrap();
        assert_eq!(kept.ecosystem, Ecosystem::Cargo);
    }

    /// A root package.json declaring `{"workspaces": ["packages/*"]}`
    /// (no pnpm-workspace.yaml anywhere) must cause `projects()` to exclude a
    /// package.json found at a path outside every workspaces glob.
    #[test]
    fn excludes_package_outside_npm_workspaces_glob() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
        std::fs::create_dir_all(root.join("tools/helper")).unwrap();
        std::fs::write(root.join("tools/helper/package.json"), r#"{"name":"helper"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects.iter().any(|p| p.path == Path::new("tools/helper")),
            "tools/helper must not be discovered, got: {projects:?}"
        );
    }

    /// Given a root with no package.json "workspaces" field but
    /// a sibling pnpm-workspace.yaml containing `packages:\n  - "packages/*"`,
    /// parsed via yaml_rust2::YamlLoader::load_from_str, a package.json at
    /// packages/kept/package.json (matching the glob) is discovered by
    /// projects(), and a package.json at tools/outside/package.json (not
    /// matching the glob) is not discovered by projects().
    #[test]
    fn pnpm_workspace_yaml_governs_npm_membership_when_no_workspaces_field() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages:\n  - \"packages/*\"\n").unwrap();
        std::fs::create_dir_all(root.join("packages/kept")).unwrap();
        std::fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(root.join("tools/outside/package.json"), r#"{"name":"outside"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/kept") && p.ecosystem == Ecosystem::Npm),
            "packages/kept must be discovered as Npm, got: {projects:?}"
        );
        assert!(
            !projects.iter().any(|p| p.path == Path::new("tools/outside")),
            "tools/outside must not be discovered, got: {projects:?}"
        );
    }

    /// Given a Cargo.toml at the workspace root with no
    /// [workspace] table at all (a single-crate repo), every Cargo candidate
    /// directory discovered by the walk is included in projects() -- the
    /// absence of a [workspace] table means no membership filter applies,
    /// not that zero packages are admitted.
    #[test]
    fn admits_all_cargo_candidates_when_root_has_no_workspace_table() {
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

    /// Given a workspace root with no Cargo.toml file at all
    /// (the file does not exist), and crates/kept/Cargo.toml elsewhere in
    /// the tree with a valid [package] table, the complete absence of a
    /// root Cargo.toml is treated identically to a root Cargo.toml with no
    /// [workspace] table: no Cargo membership filter applies, not
    /// zero packages admitted, and the walk does not error.
    #[test]
    fn admits_cargo_candidates_when_root_has_no_cargo_toml_file_at_all() {
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

    /// The root's [package]-declared crate is an implicit member
    /// exempt from the exclude list. Even a workspace root Cargo.toml whose
    /// exclude glob would textually match "." must still include the root
    /// package entry, and the members filter must still admit crates/child
    /// normally alongside the root exemption.
    #[test]
    fn includes_root_package_as_implicit_member_even_when_exclude_would_match_it() {
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

    /// An explicitly empty `members = []` is a real filter matching
    /// nothing, not a no-op. A Cargo.toml elsewhere in the tree must be
    /// excluded from the Cargo ecosystem entries.
    #[test]
    fn empty_members_array_admits_zero_cargo_entries() {
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

    /// A [workspace] table with an exclude key but no members key at
    /// all is treated identically to an absent [workspace] table for
    /// filtering purposes (every non-excluded candidate is admitted), not as
    /// an empty members = [] list which excludes everything.
    #[test]
    fn workspace_table_with_exclude_only_and_no_members_key_admits_non_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nexclude = [\"crates/foo\"]\n").unwrap();
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

    /// Given a root package.json with no "workspaces" field
    /// and no pnpm-workspace.yaml anywhere in the workspace, every Npm
    /// candidate directory discovered by the walk is included in
    /// projects() -- absence of both markers means no npm membership
    /// filter applies. This mirrors the Cargo case by asserting both
    /// the root package and the child package are admitted, not just one.
    #[test]
    fn admits_all_npm_candidates_when_no_workspaces_field_and_no_pnpm_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
        std::fs::create_dir_all(root.join("packages/child")).unwrap();
        std::fs::write(root.join("packages/child/package.json"), r#"{"name":"child"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "root package must be admitted as Npm, got: {projects:?}"
        );
        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm),
            "packages/child must be admitted as Npm, got: {projects:?}"
        );
    }

    /// Given a workspace root directory containing no
    /// package.json file at all (the file does not exist at the root) and
    /// no pnpm-workspace.yaml anywhere in the workspace, and
    /// packages/kept/package.json elsewhere in the tree with a valid
    /// "name" field, projects() includes an entry with path ==
    /// "packages/kept" and ecosystem == Ecosystem::Npm -- the complete
    /// absence of a root package.json file is treated identically to a
    /// root package.json with no "workspaces" field: no npm
    /// membership filter applies.
    #[test]
    fn admits_npm_candidates_when_root_has_no_package_json_file_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("packages/kept")).unwrap();
        std::fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/kept") && p.ecosystem == Ecosystem::Npm),
            "packages/kept must be admitted as Npm, got: {projects:?}"
        );
    }

    /// Given a root package.json with "workspaces": []
    /// (present but empty array) and at least one package.json elsewhere in
    /// the tree, projects() returns zero Npm-ecosystem entries -- an
    /// explicitly empty workspaces list is a real membership filter
    /// (matches nothing), not a no-op.
    #[test]
    fn empty_npm_workspaces_array_admits_zero_npm_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"workspaces": []}"#).unwrap();
        std::fs::create_dir_all(root.join("packages/other")).unwrap();
        std::fs::write(root.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects.iter().any(|p| p.ecosystem == Ecosystem::Npm),
            "no npm entries expected when workspaces = [], got: {projects:?}"
        );
    }

    /// A private, versionless root package.json (the standard npm/pnpm
    /// workspace-root layout, e.g. `@changesets/cli`) declares a name only to
    /// hold `workspaces` -- it must never surface as an Npm project itself,
    /// even though it would otherwise qualify as a hybrid root. The real
    /// member package is still discovered normally.
    #[test]
    fn private_versionless_root_package_json_is_not_a_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/x")).unwrap();
        std::fs::write(
            root.join("packages/x/package.json"),
            r#"{"name":"x","version":"1.0.0"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "private, versionless root must not be a project, got: {projects:?}"
        );
        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/x") && p.ecosystem == Ecosystem::Npm),
            "packages/x must still be discovered, got: {projects:?}"
        );
    }

    /// A root package.json that is private but DOES declare a version is a
    /// real, releasable hybrid-root package (e.g. a CLI's own root manifest)
    /// and must still be discovered, unlike the versionless case above.
    #[test]
    fn private_root_package_json_with_a_version_is_still_a_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","version":"0.0.0","private":true,"workspaces":["packages/*"]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/x")).unwrap();
        std::fs::write(
            root.join("packages/x/package.json"),
            r#"{"name":"x","version":"1.0.0"}"#,
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "private root with a version must still be a project, got: {projects:?}"
        );
    }

    /// The same empty "workspaces": []
    /// list must not exclude a hybrid root -- a root package.json that also
    /// declares a "name" field remains an implicit member of its own
    /// workspace and is admitted at path "." even
    /// though the empty workspaces list matches nothing.
    #[test]
    fn empty_npm_workspaces_array_still_admits_hybrid_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"name": "root-package", "workspaces": []}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/other")).unwrap();
        std::fs::write(root.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "hybrid root at '.' must still be admitted, got: {projects:?}"
        );
        assert!(
            !projects
                .iter()
                .any(|p| p.path == Path::new("packages/other") && p.ecosystem == Ecosystem::Npm),
            "non-root packages/other must remain excluded, got: {projects:?}"
        );
    }

    /// An explicitly empty
    /// "workspaces": [] list excludes a sibling package.json, whereas a root
    /// package.json with no "workspaces" field at all admits the
    /// same sibling -- proving the empty list is a real filter distinct
    /// from, and not conflated with, an absent field.
    #[test]
    fn empty_workspaces_array_distinguished_from_absent_workspaces_field() {
        let tmp_empty = tempfile::tempdir().unwrap();
        let root_empty = tmp_empty.path();
        std::fs::write(root_empty.join("package.json"), r#"{"workspaces": []}"#).unwrap();
        std::fs::create_dir_all(root_empty.join("packages/other")).unwrap();
        std::fs::write(root_empty.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();

        let tmp_absent = tempfile::tempdir().unwrap();
        let root_absent = tmp_absent.path();
        std::fs::write(root_absent.join("package.json"), r#"{"name":"root"}"#).unwrap();
        std::fs::create_dir_all(root_absent.join("packages/other")).unwrap();
        std::fs::write(root_absent.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();

        let projects_empty = IgnoreWalkLocator::new(root_empty).projects().unwrap();
        let projects_absent = IgnoreWalkLocator::new(root_absent).projects().unwrap();

        assert!(
            !projects_empty
                .iter()
                .any(|p| p.path == Path::new("packages/other") && p.ecosystem == Ecosystem::Npm),
            "empty workspaces = [] must exclude packages/other, got: {projects_empty:?}"
        );
        assert!(
            projects_absent
                .iter()
                .any(|p| p.path == Path::new("packages/other") && p.ecosystem == Ecosystem::Npm),
            "absent workspaces field must admit packages/other, got: {projects_absent:?}"
        );
    }

    /// Given a root package.json declaring
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
    fn pnpm_workspace_yaml_takes_precedence_over_package_json_workspaces_field() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages:\n  - \"tools/*\"\n").unwrap();
        std::fs::create_dir_all(root.join("tools/x")).unwrap();
        std::fs::write(root.join("tools/x/package.json"), r#"{"name":"x"}"#).unwrap();
        std::fs::create_dir_all(root.join("packages/y")).unwrap();
        std::fs::write(root.join("packages/y/package.json"), r#"{"name":"y"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("tools/x") && p.ecosystem == Ecosystem::Npm),
            "tools/x must be discovered as Npm, got: {projects:?}"
        );
        assert!(
            !projects.iter().any(|p| p.path == Path::new("packages/y")),
            "packages/y must not be discovered, got: {projects:?}"
        );
    }

    /// Consolidated Cargo workspace-membership regression group.
    /// Reassembles the individual Cargo membership fixtures, including the
    /// root's plain inclusion and exclude-exemption cases.
    #[test]
    fn cargo_workspace_membership_regression_group() {
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
            assert!(!projects.iter().any(|p| p.path == Path::new("crates/scratch-example")));
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
        // Root's own crate: plain inclusion.
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
        // exclude = ["."] still admits the root.
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
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("Cargo.toml"), "[workspace]\nexclude = [\"crates/foo\"]\n").unwrap();
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

    /// Given a pnpm-workspace.yaml declaring `packages: []`
    /// (present, empty list) and at least one package.json elsewhere in the
    /// tree, projects() returns zero Npm-ecosystem entries besides any
    /// hybrid root package (a root package.json that also
    /// declares a "name" field is still an implicit member of its own
    /// workspace and is admitted at
    /// path "." even though the empty packages: list matches nothing) --
    /// the pnpm-driven counterpart of the empty-workspaces case.
    #[test]
    fn pnpm_empty_packages_list_admits_zero_npm_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
        std::fs::create_dir_all(root.join("packages/other")).unwrap();
        std::fs::write(root.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            !projects.iter().any(|p| p.ecosystem == Ecosystem::Npm),
            "no non-root npm entries expected, got: {projects:?}"
        );
    }

    /// The same empty `packages: []`
    /// list must not exclude a hybrid root -- a root package.json that
    /// also declares a "name" field remains an implicit member of its own
    /// workspace and is admitted at
    /// path "." even though the empty packages: list matches nothing.
    #[test]
    fn pnpm_empty_packages_list_still_admits_hybrid_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
        std::fs::write(root.join("package.json"), r#"{"name":"root-pkg"}"#).unwrap();
        std::fs::create_dir_all(root.join("packages/other")).unwrap();
        std::fs::write(root.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "hybrid root at '.' must still be admitted, got: {projects:?}"
        );
        assert!(
            !projects
                .iter()
                .any(|p| p.path == Path::new("packages/other") && p.ecosystem == Ecosystem::Npm),
            "non-root packages/other must remain excluded, got: {projects:?}"
        );
    }

    /// A sibling pnpm-workspace.yaml that fails to parse as YAML
    /// does not count as "present" for precedence purposes -- the
    /// root package.json's "workspaces" field is still consulted and
    /// governs npm-ecosystem membership normally.
    #[test]
    fn malformed_pnpm_yaml_does_not_count_as_present_for_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages: [\"packages/*\"\n").unwrap();
        std::fs::create_dir_all(root.join("packages/y")).unwrap();
        std::fs::write(root.join("packages/y/package.json"), r#"{"name":"y"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects.iter().any(|p| p.path == Path::new("packages/y")),
            "package.json workspaces field must still govern membership when \
             pnpm-workspace.yaml is malformed YAML, got: {projects:?}"
        );
    }

    /// A root package.json with no "workspaces" field, alongside a
    /// pnpm-workspace.yaml whose YAML is malformed (unterminated flow
    /// sequence -> yaml_rust2::YamlLoader::load_from_str returns Err), must
    /// not panic or error the whole walk. The npm membership filter is
    /// treated as absent for that workspace (falls back to
    /// admit-all), matching the malformed-TOML treatment.
    #[test]
    fn malformed_pnpm_yaml_with_no_package_json_workspaces_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages: [\"packages/*\"\n").unwrap();
        std::fs::create_dir_all(root.join("packages/kept")).unwrap();
        std::fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects.iter().any(|p| p.path == Path::new("packages/kept")),
            "malformed pnpm-workspace.yaml with no package.json workspaces field \
             must fall back to admit-all npm membership, got: {projects:?}"
        );
    }

    /// Consolidated npm/pnpm workspace-membership regression group.
    /// Reassembles the individual npm and pnpm membership fixtures into a
    /// single regression test.
    #[test]
    fn npm_pnpm_workspace_membership_regression_group() {
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
            std::fs::create_dir_all(root.join("tools/helper")).unwrap();
            std::fs::write(root.join("tools/helper/package.json"), r#"{"name":"helper"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(!projects.iter().any(|p| p.path == Path::new("tools/helper")));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("pnpm-workspace.yaml"), "packages:\n  - \"packages/*\"\n").unwrap();
            std::fs::create_dir_all(root.join("packages/kept")).unwrap();
            std::fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
            std::fs::create_dir_all(root.join("tools/outside")).unwrap();
            std::fs::write(root.join("tools/outside/package.json"), r#"{"name":"outside"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects.iter().any(|p| p.path == Path::new("packages/kept")));
            assert!(!projects.iter().any(|p| p.path == Path::new("tools/outside")));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
            std::fs::create_dir_all(root.join("packages/child")).unwrap();
            std::fs::write(root.join("packages/child/package.json"), r#"{"name":"child"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::create_dir_all(root.join("packages/kept")).unwrap();
            std::fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects.iter().any(|p| p.path == Path::new("packages/kept")));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
            std::fs::write(root.join("pnpm-workspace.yaml"), "packages:\n  - \"tools/*\"\n").unwrap();
            std::fs::create_dir_all(root.join("tools/x")).unwrap();
            std::fs::write(root.join("tools/x/package.json"), r#"{"name":"x"}"#).unwrap();
            std::fs::create_dir_all(root.join("packages/y")).unwrap();
            std::fs::write(root.join("packages/y/package.json"), r#"{"name":"y"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects.iter().any(|p| p.path == Path::new("tools/x")));
            assert!(!projects.iter().any(|p| p.path == Path::new("packages/y")));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
            std::fs::create_dir_all(root.join("packages/other")).unwrap();
            std::fs::write(root.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(!projects.iter().any(|p| p.ecosystem == Ecosystem::Npm));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
            std::fs::write(root.join("pnpm-workspace.yaml"), "packages: [\"packages/*\"\n").unwrap();
            std::fs::create_dir_all(root.join("packages/y")).unwrap();
            std::fs::write(root.join("packages/y/package.json"), r#"{"name":"y"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects.iter().any(|p| p.path == Path::new("packages/y")));
        }

        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("package.json"), r#"{"workspaces": []}"#).unwrap();
            std::fs::create_dir_all(root.join("packages/other")).unwrap();
            std::fs::write(root.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(!projects.iter().any(|p| p.ecosystem == Ecosystem::Npm));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("package.json"),
                r#"{"name": "root-package", "workspaces": ["packages/*"]}"#,
            )
            .unwrap();
            std::fs::create_dir_all(root.join("packages/child")).unwrap();
            std::fs::write(root.join("packages/child/package.json"), r#"{"name":"child"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm));
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("package.json"),
                r#"{"name": "root-package", "workspaces": ["packages/*"]}"#,
            )
            .unwrap();
            std::fs::write(root.join("pnpm-workspace.yaml"), "packages: [\"packages/*\"\n").unwrap();
            std::fs::create_dir_all(root.join("packages/child")).unwrap();
            std::fs::write(root.join("packages/child/package.json"), r#"{"name":"child"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm));
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(
                root.join("package.json"),
                r#"{"name": "root-package", "workspaces": []}"#,
            )
            .unwrap();
            std::fs::create_dir_all(root.join("packages/other")).unwrap();
            std::fs::write(root.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            let npm_entries: Vec<_> = projects.iter().filter(|p| p.ecosystem == Ecosystem::Npm).collect();
            assert_eq!(npm_entries.len(), 1);
            assert_eq!(npm_entries[0].path, Path::new("."));
        }
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::write(root.join("package.json"), r#"{"name": "root-package"}"#).unwrap();
            std::fs::write(root.join("pnpm-workspace.yaml"), "packages:\n  - \"packages/*\"\n").unwrap();
            std::fs::create_dir_all(root.join("packages/child")).unwrap();
            std::fs::write(root.join("packages/child/package.json"), r#"{"name":"child"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm));
            assert!(projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm));
        }
    }

    /// Given a root package.json declaring both "name" and
    /// "workspaces": ["packages/*"] (the ordinary npm/yarn-classic
    /// monorepo root layout), and no pnpm-workspace.yaml anywhere,
    /// IgnoreWalkLocator::new(root).projects() returns an entry for the
    /// root itself (path == ".", ecosystem == Ecosystem::Npm) in addition
    /// to entries discovered under packages/* -- the root package is never
    /// silently dropped merely because it matches no entry in its own
    /// "workspaces" glob list.
    #[test]
    fn npm_hybrid_root_admitted_when_workspaces_field_governs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"name": "root-package", "workspaces": ["packages/*"]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/child")).unwrap();
        std::fs::write(root.join("packages/child/package.json"), r#"{"name":"child"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "root package must be admitted as Npm, got: {projects:?}"
        );
        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm),
            "packages/child must be admitted as Npm, got: {projects:?}"
        );
    }

    /// Given a root package.json declaring both "name" and
    /// "workspaces": ["packages/*"], and a sibling pnpm-workspace.yaml whose
    /// content is malformed YAML, calling projects() still returns an entry
    /// with path == "." and ecosystem == Npm for the root's own package.
    /// A malformed pnpm-workspace.yaml does not count as "present" for
    /// precedence purposes, so package.json's "workspaces"
    /// field governs and the hybrid-root exemption applies exactly as it
    /// would if no pnpm-workspace.yaml existed at all.
    #[test]
    fn npm_hybrid_root_admitted_when_sibling_pnpm_workspace_yaml_is_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"name": "root-package", "workspaces": ["packages/*"]}"#,
        )
        .unwrap();
        // Deliberately malformed YAML — yaml_rust2::YamlLoader::load_from_str
        // must return Err(..) for this content.
        std::fs::write(root.join("pnpm-workspace.yaml"), b": {\x00 invalid yaml \xff\xfe").unwrap();
        std::fs::create_dir_all(root.join("packages/child")).unwrap();
        std::fs::write(root.join("packages/child/package.json"), r#"{"name":"child"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "root package must be admitted as Npm despite malformed pnpm-workspace.yaml, got: {projects:?}"
        );
        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm),
            "packages/child must be admitted as Npm, got: {projects:?}"
        );
    }

    /// Given a root package.json declaring "name": "root-package" and a
    /// sibling pnpm-workspace.yaml that exists, parses successfully, and
    /// declares packages: ["packages/*"] (a glob that does not itself match
    /// "."), calling projects() returns an entry with path == "." and
    /// ecosystem == Npm for the root's own package, in addition to any entries
    /// discovered under packages/* — the root package's identity is never
    /// silently dropped merely because the governing pnpm-workspace.yaml's
    /// packages: list contains no entry matching ".".
    #[test]
    fn npm_hybrid_root_admitted_when_pnpm_workspace_yaml_governs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"name": "root-package"}"#).unwrap();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages:\n  - \"packages/*\"\n").unwrap();
        std::fs::create_dir_all(root.join("packages/child")).unwrap();
        std::fs::write(root.join("packages/child/package.json"), r#"{"name":"child"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new(".") && p.ecosystem == Ecosystem::Npm),
            "root package must be admitted as Npm when pnpm-workspace.yaml governs, got: {projects:?}"
        );
        assert!(
            projects
                .iter()
                .any(|p| p.path == Path::new("packages/child") && p.ecosystem == Ecosystem::Npm),
            "packages/child must be admitted as Npm, got: {projects:?}"
        );
    }

    /// Given a root package.json declaring both "name": "root-package"
    /// and "workspaces": [] (present, empty array) and at least one other
    /// package.json elsewhere in the tree, calling projects() returns exactly
    /// one Npm-ecosystem entry: path == "." for the root's own package.
    /// The empty workspaces list excludes every non-root candidate but never
    /// excludes the root's own package.
    #[test]
    fn npm_hybrid_root_admitted_despite_empty_workspaces_array() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"name": "root-package", "workspaces": []}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("packages/child")).unwrap();
        std::fs::write(root.join("packages/child/package.json"), r#"{"name":"child"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        let npm_projects: Vec<_> = projects.iter().filter(|p| p.ecosystem == Ecosystem::Npm).collect();

        assert_eq!(
            npm_projects.len(),
            1,
            "empty workspaces array must admit exactly one Npm entry (the hybrid root), got: {npm_projects:?}"
        );
        assert_eq!(
            npm_projects[0].path,
            Path::new("."),
            "the single Npm entry must be the root package at path '.', got: {npm_projects:?}"
        );
    }

    #[test]
    fn malformed_cargo_members_bare_string_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = \"crates/*\"\n").unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(
            root.join("tools/outside/Cargo.toml"),
            "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects.iter().any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn malformed_cargo_members_non_string_entry_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/*\", 42]\n").unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(
            root.join("tools/outside/Cargo.toml"),
            "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects.iter().any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn malformed_root_cargo_toml_syntax_falls_back_to_absent_cargo_filter() {
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
    fn malformed_cargo_exclude_bare_string_falls_back_to_excluding_nothing() {
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

        assert!(projects.iter().any(|p| p.path == Path::new("crates/scratch-example")));
        assert!(projects.iter().any(|p| p.path == Path::new("crates/kept-example")));
        assert!(!projects.iter().any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn malformed_cargo_exclude_non_string_entry_falls_back_to_excluding_nothing() {
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

        assert!(projects.iter().any(|p| p.path == Path::new("crates/scratch-example")));
        assert!(projects.iter().any(|p| p.path == Path::new("crates/kept-example")));
        assert!(!projects.iter().any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn invalid_glob_in_cargo_members_is_skipped_without_disabling_sibling_entries() {
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

        assert!(projects.iter().any(|p| p.path == Path::new("crates/kept-example")));
    }

    #[test]
    fn invalid_glob_in_npm_workspaces_is_skipped_without_disabling_sibling_entries() {
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

        assert!(projects.iter().any(|p| p.path == Path::new("packages/kept-example")));
    }

    #[test]
    fn invalid_glob_in_cargo_exclude_is_skipped_without_disabling_sibling_entries() {
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

        assert!(!projects.iter().any(|p| p.path == Path::new("crates/scratch-example")));
        assert!(projects.iter().any(|p| p.path == Path::new("crates/kept-example")));
    }

    #[test]
    fn invalid_glob_in_pnpm_packages_is_skipped_without_disabling_sibling_entries() {
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

        assert!(projects.iter().any(|p| p.path == Path::new("packages/kept-example")));
    }

    #[test]
    fn malformed_npm_workspaces_bare_string_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"workspaces": "packages/*"}"#).unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(root.join("tools/outside/package.json"), r#"{"name":"outside"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects.iter().any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn malformed_npm_workspaces_non_string_entry_falls_back_to_absent_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), r#"{"workspaces": ["packages/*", 42]}"#).unwrap();
        std::fs::create_dir_all(root.join("tools/outside")).unwrap();
        std::fs::write(root.join("tools/outside/package.json"), r#"{"name":"outside"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects.iter().any(|p| p.path == Path::new("tools/outside")));
    }

    #[test]
    fn malformed_root_package_json_syntax_falls_back_to_absent_npm_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), "{\"workspaces\": [\"packages/*\"],}").unwrap();
        std::fs::create_dir_all(root.join("packages/kept")).unwrap();
        std::fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();

        assert!(projects.iter().any(|p| p.path == Path::new("packages/kept")));
    }

    /// A pnpm-workspace.yaml that parses successfully but
    /// whose top-level document either has no `packages:` key, or has a
    /// `packages:` key whose value is not a YAML sequence of strings --
    /// e.g. `packages: "packages/*"` (bare scalar), `packages: {foo: bar}`
    /// (mapping), or a sequence containing a non-string element -- does not
    /// panic; `IgnoreWalkLocator::projects()` treats the npm membership
    /// filter as absent for that workspace, the same fallback as the
    /// YAML-syntax-error case.
    #[test]
    fn pnpm_packages_wrong_shape_falls_back_to_absent_filter() {
        let dir_missing = tempfile::tempdir().unwrap();
        let root_missing = dir_missing.path();
        std::fs::write(root_missing.join("pnpm-workspace.yaml"), "other_key: true\n").unwrap();
        std::fs::create_dir_all(root_missing.join("packages/kept")).unwrap();
        std::fs::write(root_missing.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
        let projects_missing = IgnoreWalkLocator::new(root_missing).projects().unwrap();
        assert!(
            projects_missing.iter().any(|p| p.path == Path::new("packages/kept")),
            "missing key: packages/kept must be admitted, got: {projects_missing:?}"
        );

        let dir_scalar = tempfile::tempdir().unwrap();
        let root_scalar = dir_scalar.path();
        std::fs::write(root_scalar.join("pnpm-workspace.yaml"), "packages: \"packages/*\"\n").unwrap();
        std::fs::create_dir_all(root_scalar.join("packages/kept")).unwrap();
        std::fs::write(root_scalar.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
        let projects_scalar = IgnoreWalkLocator::new(root_scalar).projects().unwrap();
        assert!(
            projects_scalar.iter().any(|p| p.path == Path::new("packages/kept")),
            "wrong scalar type: packages/kept must be admitted, got: {projects_scalar:?}"
        );

        let dir_mapping = tempfile::tempdir().unwrap();
        let root_mapping = dir_mapping.path();
        std::fs::write(root_mapping.join("pnpm-workspace.yaml"), "packages:\n  foo: bar\n").unwrap();
        std::fs::create_dir_all(root_mapping.join("packages/kept")).unwrap();
        std::fs::write(root_mapping.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
        let projects_mapping = IgnoreWalkLocator::new(root_mapping).projects().unwrap();
        assert!(
            projects_mapping.iter().any(|p| p.path == Path::new("packages/kept")),
            "mapping value: packages/kept must be admitted, got: {projects_mapping:?}"
        );

        let dir_nonstring = tempfile::tempdir().unwrap();
        let root_nonstring = dir_nonstring.path();
        std::fs::write(
            root_nonstring.join("pnpm-workspace.yaml"),
            "packages:\n  - \"a\"\n  - 42\n",
        )
        .unwrap();
        std::fs::create_dir_all(root_nonstring.join("packages/kept")).unwrap();
        std::fs::write(root_nonstring.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
        let projects_nonstring = IgnoreWalkLocator::new(root_nonstring).projects().unwrap();
        assert!(
            projects_nonstring.iter().any(|p| p.path == Path::new("packages/kept")),
            "non-string sequence element: packages/kept must be admitted, got: {projects_nonstring:?}"
        );
    }

    /// A pnpm-workspace.yaml file that is empty (zero bytes),
    /// contains only whitespace, or contains only YAML comments -- such that
    /// yaml_rust2::YamlLoader::load_from_str returns Ok(vec![]), a
    /// successful parse producing zero YAML documents rather than a single
    /// document -- does not panic (IgnoreWalkLocator::projects() must not
    /// unconditionally index docs[0] or call docs.first().unwrap()); it
    /// treats the npm membership filter as absent for that workspace, the
    /// same fallback as the missing-packages-key case.
    #[test]
    fn zero_yaml_documents_falls_back_to_absent_filter_without_panic() {
        let dir_empty = tempfile::tempdir().unwrap();
        let root_empty = dir_empty.path();
        std::fs::write(root_empty.join("pnpm-workspace.yaml"), "").unwrap();
        std::fs::create_dir_all(root_empty.join("packages/kept")).unwrap();
        std::fs::write(root_empty.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
        let projects_empty = IgnoreWalkLocator::new(root_empty).projects().unwrap();
        assert!(
            projects_empty.iter().any(|p| p.path == Path::new("packages/kept")),
            "zero bytes: packages/kept must be admitted, got: {projects_empty:?}"
        );

        let dir_whitespace = tempfile::tempdir().unwrap();
        let root_whitespace = dir_whitespace.path();
        std::fs::write(root_whitespace.join("pnpm-workspace.yaml"), "   \n\t\n  \n").unwrap();
        std::fs::create_dir_all(root_whitespace.join("packages/kept")).unwrap();
        std::fs::write(root_whitespace.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
        let projects_whitespace = IgnoreWalkLocator::new(root_whitespace).projects().unwrap();
        assert!(
            projects_whitespace.iter().any(|p| p.path == Path::new("packages/kept")),
            "whitespace only: packages/kept must be admitted, got: {projects_whitespace:?}"
        );

        let dir_comments = tempfile::tempdir().unwrap();
        let root_comments = dir_comments.path();
        std::fs::write(
            root_comments.join("pnpm-workspace.yaml"),
            "# no packages here\n# another comment\n",
        )
        .unwrap();
        std::fs::create_dir_all(root_comments.join("packages/kept")).unwrap();
        std::fs::write(root_comments.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
        let projects_comments = IgnoreWalkLocator::new(root_comments).projects().unwrap();
        assert!(
            projects_comments.iter().any(|p| p.path == Path::new("packages/kept")),
            "comments only: packages/kept must be admitted, got: {projects_comments:?}"
        );
    }

    /// Regression-proof consolidation of the Cargo
    /// malformed-input criteria already proven individually (bare-string members/exclude
    /// fallback, malformed root `Cargo.toml` fallback, invalid glob
    /// skip-without-disabling for both members and exclude). Reassembles
    /// those fixtures into one regression group so future changes
    /// to Cargo membership handling that break any of these
    /// no-panic/fallback behaviors are caught here.
    #[test]
    fn cargo_malformed_edge_case_regression_slice() {
        // Bare-string `members` falls back to absent filter (admits everything).
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = \"crates/*\"\n").unwrap();
            fs::create_dir_all(root.join("tools/outside")).unwrap();
            fs::write(
                root.join("tools/outside/Cargo.toml"),
                "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects.iter().any(|p| p.path == Path::new("tools/outside")),
                "tools/outside must be admitted, got: {projects:?}"
            );
        }

        // Bare-string `exclude` falls back to excluding nothing, while
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
                projects.iter().any(|p| p.path == Path::new("crates/scratch-example")),
                "crates/scratch-example must be admitted, got: {projects:?}"
            );
            assert!(
                projects.iter().any(|p| p.path == Path::new("crates/kept-example")),
                "crates/kept-example must be admitted, got: {projects:?}"
            );
            assert!(
                !projects.iter().any(|p| p.path == Path::new("tools/outside")),
                "tools/outside must remain excluded by members, got: {projects:?}"
            );
        }

        // Syntactically invalid root TOML falls back to absent Cargo filter.
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
                "crates/kept must be admitted, got: {projects:?}"
            );
        }

        // An invalid glob entry in `members` is skipped without
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
                projects.iter().any(|p| p.path == Path::new("crates/kept-example")),
                "crates/kept-example must be admitted, got: {projects:?}"
            );
        }

        // An invalid glob entry in `exclude` is skipped without
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
                !projects.iter().any(|p| p.path == Path::new("crates/scratch-example")),
                "crates/scratch-example must remain excluded, got: {projects:?}"
            );
            assert!(
                projects.iter().any(|p| p.path == Path::new("crates/kept-example")),
                "crates/kept-example must be admitted, got: {projects:?}"
            );
        }
    }

    /// Consolidated regression group reassembling the
    /// fixtures already proven individually (malformed-workspaces
    /// fallback, malformed-root-json fallback, invalid-glob
    /// skip, empty-workspaces-array excludes everything).
    /// Reassembles those fixtures into one regression group so
    /// future changes to npm membership handling that break any of these
    /// no-panic/fallback behaviors are caught here.
    #[test]
    fn npm_malformed_edge_case_regression_slice() {
        // Bare-string `workspaces` falls back to absent npm filter
        // (admits everything, including paths outside any glob).
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("package.json"), r#"{"workspaces": "packages/*"}"#).unwrap();
            fs::create_dir_all(root.join("tools/outside")).unwrap();
            fs::write(root.join("tools/outside/package.json"), r#"{"name":"outside"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects.iter().any(|p| p.path == Path::new("tools/outside")),
                "tools/outside must be admitted, got: {projects:?}"
            );
        }

        // Syntactically invalid JSON falls back to absent npm
        // filter without panicking or erroring the whole walk.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("package.json"), "{\"workspaces\": [\"packages/*\"],}").unwrap();
            fs::create_dir_all(root.join("packages/kept")).unwrap();
            fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects.iter().any(|p| p.path == Path::new("packages/kept")),
                "packages/kept must be admitted, got: {projects:?}"
            );
        }

        // An invalid glob entry in npm `workspaces` is skipped
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
                projects.iter().any(|p| p.path == Path::new("packages/kept-example")),
                "packages/kept-example must be admitted, got: {projects:?}"
            );
        }

        // An explicitly empty `workspaces` array is a real
        // membership filter that matches nothing (distinguished from an
        // absent `workspaces` field).
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("package.json"), r#"{"workspaces": []}"#).unwrap();
            fs::create_dir_all(root.join("packages/other")).unwrap();
            fs::write(root.join("packages/other/package.json"), r#"{"name":"other"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                !projects.iter().any(|p| p.ecosystem == Ecosystem::Npm),
                "no Npm entries expected, got: {projects:?}"
            );
        }
    }

    /// Consolidated regression group reassembling the
    /// fixtures already proven individually (invalid-glob skip,
    /// malformed-yaml-no-workspaces fallback, wrong-shape fallback,
    /// zero-yaml-documents fallback). Reassembles those fixtures into one
    /// regression group so future changes to pnpm membership handling
    /// that break any of these no-panic/fallback behaviors are caught
    /// here.
    #[test]
    fn pnpm_malformed_edge_case_regression_slice() {
        // An invalid glob entry in pnpm `packages` is skipped
        // without disabling sibling well-formed entries.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(
                root.join("pnpm-workspace.yaml"),
                "packages:\n  - \"packages/kept-example\"\n  - \"packages/[unterminated\"\n",
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
                projects.iter().any(|p| p.path == Path::new("packages/kept-example")),
                "packages/kept-example must be admitted, got: {projects:?}"
            );
        }

        // Malformed pnpm-workspace.yaml (unterminated flow sequence)
        // with a root package.json present but lacking a "workspaces"
        // field falls back to admit-all npm membership without panicking
        // or erroring the whole walk.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
            fs::write(root.join("pnpm-workspace.yaml"), "packages: [\"packages/*\"\n").unwrap();
            fs::create_dir_all(root.join("packages/kept")).unwrap();
            fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects.iter().any(|p| p.path == Path::new("packages/kept")),
                "packages/kept must be admitted, got: {projects:?}"
            );
        }

        // Pnpm-workspace.yaml parses successfully but `packages:`
        // is a bare scalar string, not a sequence -- falls back to absent
        // npm membership filter without panicking.
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("pnpm-workspace.yaml"), "packages: \"packages/*\"\n").unwrap();
            fs::create_dir_all(root.join("packages/kept")).unwrap();
            fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects.iter().any(|p| p.path == Path::new("packages/kept")),
                "packages/kept must be admitted, got: {projects:?}"
            );
        }

        // Pnpm-workspace.yaml containing only YAML comments parses
        // successfully to zero documents (Ok(vec![])) -- falls back to
        // absent npm membership filter without unconditionally indexing
        // docs[0].
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("pnpm-workspace.yaml"), "# no packages here\n").unwrap();
            fs::create_dir_all(root.join("packages/kept")).unwrap();
            fs::write(root.join("packages/kept/package.json"), r#"{"name":"kept"}"#).unwrap();
            let projects = IgnoreWalkLocator::new(root).projects().unwrap();
            assert!(
                projects.iter().any(|p| p.path == Path::new("packages/kept")),
                "packages/kept must be admitted, got: {projects:?}"
            );
        }
    }

    #[test]
    fn unparseable_manifest_warning_names_path_and_message() {
        let msg = unparseable_manifest_warning(Path::new("tools/scratch/package.json"), "trailing comma");
        assert_eq!(
            msg,
            "warning: failed to parse manifest `tools/scratch/package.json`: trailing comma"
        );
    }

    /// A manifest admitted by an explicit workspace-membership entry that fails to parse must
    /// error with a coded, path-naming diagnostic instead of being silently dropped.
    #[test]
    fn unparseable_manifest_explicitly_admitted_by_its_own_ecosystem_errors() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"a\", \"b\"]\n").unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(
            root.join("a/Cargo.toml"),
            "[package]\nname = \"a\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(
            root.join("b/Cargo.toml"),
            "[package]\nname = \"b\"\nversion = \"1.0.0\"\n<<<<<<< HEAD\n",
        )
        .unwrap();

        let err = IgnoreWalkLocator::new(root).projects().unwrap_err();
        assert!(
            matches!(&err, LocateError::ManifestParseError { path, .. } if path.ends_with("b/Cargo.toml")),
            "expected ManifestParseError naming b/Cargo.toml, got: {err:?}"
        );
    }

    /// A manifest that fails to parse and shares its directory with a manifest of a *different*
    /// ecosystem that IS admitted must also error, even though its own ecosystem's membership
    /// (npm workspaces) doesn't separately name it.
    #[test]
    fn unparseable_manifest_sharing_a_directory_with_an_admitted_sibling_errors() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"a\", \"b\"]\n").unwrap();
        fs::write(root.join("package.json"), r#"{"private":true,"workspaces":["a"]}"#).unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(
            root.join("a/Cargo.toml"),
            "[package]\nname = \"a\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(root.join("a/package.json"), r#"{"name":"a","version":"1.0.0",}"#).unwrap();

        let err = IgnoreWalkLocator::new(root).projects().unwrap_err();
        assert!(
            matches!(&err, LocateError::ManifestParseError { path, .. } if path.ends_with("a/package.json")),
            "expected ManifestParseError naming a/package.json, got: {err:?}"
        );
    }

    /// A manifest that fails to parse, is admitted by no explicit membership, and shares no
    /// directory with an admitted manifest is a non-fatal, silently-skipped warning -- matching
    /// the long-standing malformed-root-manifest fallback tests above.
    #[test]
    fn unparseable_manifest_outside_any_membership_is_skipped_not_fatal() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"kept\"]\n").unwrap();
        fs::create_dir_all(root.join("kept")).unwrap();
        fs::write(
            root.join("kept/Cargo.toml"),
            "[package]\nname = \"kept\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("scratch")).unwrap();
        fs::write(root.join("scratch/Cargo.toml"), "[package\nname = \"broken\"\n").unwrap();

        let projects = IgnoreWalkLocator::new(root).projects().unwrap();
        assert!(
            projects.iter().any(|p| p.path == Path::new("kept")),
            "non-broken admitted member must still be discovered, got: {projects:?}"
        );
        assert!(
            !projects.iter().any(|p| p.path == Path::new("scratch")),
            "the unparseable, non-admitted manifest must not appear as a discovered project, got: {projects:?}"
        );
    }
}
