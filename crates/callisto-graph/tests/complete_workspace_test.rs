//! A loaded workspace is complete or it is an error: a manifest the graph build cannot resolve must never shrink the
//! release roster quietly.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use callisto_graph::locate::{IgnoreWalkLocator, LocateError, ProjectLocator};
use callisto_graph::resolver::ManifestWalkResolver;
use callisto_graph::{DependencyResolver, GraphError, Workspace};
use callisto_model::{
    CommandError, CommandOutput, CommandRunner, DiagnosticCode, DiagnosticSeverity, Ecosystem, PackageId, ProjectRoot,
};

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// Reports fixed projects, as a locator would that raced with a change on disk.
struct Fixed(Vec<ProjectRoot>);

impl ProjectLocator for Fixed {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError> {
        Ok(self.0.clone())
    }
}

/// Like [`Fixed`], plus npm directories offered only as platform-package candidates.
struct WithCandidates(Vec<ProjectRoot>, Vec<ProjectRoot>);

impl ProjectLocator for WithCandidates {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError> {
        Ok(self.0.clone())
    }

    fn projects_and_platform_candidates(&self) -> Result<(Vec<ProjectRoot>, Vec<ProjectRoot>), LocateError> {
        Ok((self.0.clone(), self.1.clone()))
    }
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn unresolved_of(error: GraphError) -> Vec<String> {
    match error {
        GraphError::IncompleteWorkspace { unresolved } => unresolved,
        other => panic!("expected IncompleteWorkspace, got {other:?}"),
    }
}

#[test]
fn a_clean_multi_ecosystem_workspace_loads_with_every_package() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\n",
    );
    write(root, "web/package.json", r#"{"name":"web","version":"1.0.0"}"#);
    write(
        root,
        "py/pyproject.toml",
        "[project]\nname = \"py-service\"\nversion = \"0.2.0\"\n",
    );

    let workspace = Workspace::load(root.to_path_buf(), &IgnoreWalkLocator::new(root), &NoopRunner).unwrap();

    let mut names: Vec<String> = workspace
        .graph
        .packages()
        .map(|package| package.id.name().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["core", "py-service", "web"]);
    assert!(workspace.require_complete().is_ok());
}

/// Rust workspaces living in subdirectories each govern their own members: a member inherits from its own, not from a sibling's.
#[test]
fn a_cargo_workspace_below_the_root_resolves_the_inheritance_of_its_members() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "rust/Cargo.toml",
        "[workspace]\nmembers = [\"top\"]\n[workspace.package]\nversion = \"9.9.9\"\n",
    );
    write(
        root,
        "rust/top/Cargo.toml",
        "[package]\nname = \"top\"\nversion.workspace = true\n",
    );
    write(
        root,
        "native/Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n[workspace.package]\nversion = \"1.0.0\"\n[workspace.dependencies]\n\
         a = { path = \"crates/a\", version = \"1.0.0\" }\n",
    );
    write(
        root,
        "native/crates/a/Cargo.toml",
        "[package]\nname = \"a\"\nversion.workspace = true\n",
    );
    write(
        root,
        "native/crates/b/Cargo.toml",
        "[package]\nname = \"b\"\nversion = \"0.2.0\"\n[dependencies]\na = { workspace = true }\n",
    );

    let locator = IgnoreWalkLocator::new(root);
    let workspace = Workspace::load(root.to_path_buf(), &locator, &NoopRunner).unwrap();

    let versions: BTreeMap<String, String> = workspace
        .base_versions()
        .unwrap()
        .into_iter()
        .map(|(id, version)| (id.name().to_string(), version.render().to_string()))
        .collect();
    assert_eq!(
        versions,
        BTreeMap::from([
            ("a".to_string(), "1.0.0".to_string()),
            ("b".to_string(), "0.2.0".to_string()),
            ("top".to_string(), "9.9.9".to_string()),
        ]),
    );
    let b = PackageId::parse("b").unwrap();
    let edges: Vec<_> = workspace.graph.dependencies_of(&b).collect();
    assert_eq!(edges.len(), 1, "{edges:?}");
    assert!(edges[0].inherited);
    assert_eq!(edges[0].from_manifest, Path::new("native/Cargo.toml"));
}

/// A member that inherits from a workspace no manifest above it declares has no inheritance to resolve against. Its
/// publish targets and dependency edges are unknown, so the workspace is not complete.
#[test]
fn a_member_whose_inheritance_cannot_be_resolved_makes_the_workspace_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "native/Cargo.toml",
        "[package]\nname = \"native\"\nversion = \"1.0.0\"\n",
    );
    write(
        root,
        "native/crates/a/Cargo.toml",
        "[package]\nname = \"a\"\nversion.workspace = true\n",
    );

    let locator = IgnoreWalkLocator::new(root);
    let error = Workspace::load(root.to_path_buf(), &locator, &NoopRunner)
        .err()
        .unwrap();
    let unresolved = unresolved_of(error);
    assert_eq!(unresolved.len(), 1, "one entry per manifest: {unresolved:?}");
    assert!(unresolved[0].contains("native/crates/a/Cargo.toml"), "{unresolved:?}");
    assert!(
        unresolved[0].contains("no Cargo workspace governs it"),
        "{unresolved:?}"
    );

    // The walk itself records the manifest as an error-level diagnostic, once per stage that could not open it.
    let config = callisto_graph::load_config(root).unwrap();
    let cache = RefCell::new(BTreeMap::new());
    let graph = ManifestWalkResolver::build(root, &locator, &NoopRunner, &config, &cache).unwrap();
    let recorded: Vec<_> = graph
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == DiagnosticCode::ManifestUnresolved)
        .collect();
    assert!(!recorded.is_empty());
    assert!(recorded
        .iter()
        .all(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error));
}

/// The same failure for each ecosystem, injected as a manifest that vanished after discovery reported it.
#[test]
fn a_manifest_that_disappears_after_discovery_is_an_error_for_every_ecosystem() {
    for (ecosystem, name) in [
        (Ecosystem::Cargo, "gone-cargo"),
        (Ecosystem::Npm, "gone-npm"),
        (Ecosystem::Pypi, "gone-py"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "kept/package.json", r#"{"name":"kept","version":"1.0.0"}"#);
        fs::create_dir_all(root.join(name)).unwrap();
        let locator = Fixed(vec![
            ProjectRoot {
                id: PackageId::Bare("kept".to_string()),
                path: PathBuf::from("kept"),
                ecosystem: Ecosystem::Npm,
            },
            ProjectRoot {
                id: PackageId::Bare(name.to_string()),
                path: PathBuf::from(name),
                ecosystem,
            },
        ]);

        let error = Workspace::load(root.to_path_buf(), &locator, &NoopRunner)
            .err()
            .unwrap_or_else(|| panic!("{ecosystem:?}: a workspace missing a manifest must not load"));
        let unresolved = unresolved_of(error);
        assert_eq!(
            unresolved.len(),
            1,
            "{ecosystem:?}: one entry per manifest: {unresolved:?}"
        );
        assert!(unresolved[0].contains(name), "{ecosystem:?}: {unresolved:?}");
    }
}

/// A package at the workspace root has the path `.`, so its manifest must still be named once, as `package.json`.
#[test]
fn a_root_level_manifest_that_disappears_is_named_once_without_a_dot_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let locator = Fixed(vec![ProjectRoot {
        id: PackageId::Bare("root-pkg".to_string()),
        path: PathBuf::from("."),
        ecosystem: Ecosystem::Npm,
    }]);

    let unresolved = unresolved_of(
        Workspace::load(root.to_path_buf(), &locator, &NoopRunner)
            .err()
            .unwrap(),
    );
    assert_eq!(unresolved.len(), 1, "{unresolved:?}");
    assert!(unresolved[0].starts_with("`package.json`"), "{unresolved:?}");
}

/// A platform-package candidate outside the workspace whose manifest cannot be opened is reported too, not skipped.
#[test]
fn an_unreadable_platform_package_candidate_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "kept/package.json", r#"{"name":"kept","version":"1.0.0"}"#);
    fs::create_dir_all(root.join("platform-linux")).unwrap();
    let locator = WithCandidates(
        vec![ProjectRoot {
            id: PackageId::Bare("kept".to_string()),
            path: PathBuf::from("kept"),
            ecosystem: Ecosystem::Npm,
        }],
        vec![ProjectRoot {
            id: PackageId::Bare("platform-linux".to_string()),
            path: PathBuf::from("platform-linux"),
            ecosystem: Ecosystem::Npm,
        }],
    );

    let unresolved = unresolved_of(
        Workspace::load(root.to_path_buf(), &locator, &NoopRunner)
            .err()
            .unwrap(),
    );
    assert_eq!(unresolved.len(), 1, "{unresolved:?}");
    assert!(unresolved[0].contains("platform-linux"), "{unresolved:?}");
}
