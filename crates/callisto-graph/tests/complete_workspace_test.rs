//! A loaded workspace is complete or it is an error: a manifest the graph build cannot resolve must never shrink the
//! release roster quietly.

use std::fs;
use std::path::{Path, PathBuf};

use callisto_graph::locate::{IgnoreWalkLocator, LocateError, ProjectLocator};
use callisto_graph::{DependencyResolver, GraphError, Workspace};
use callisto_model::{CommandError, CommandOutput, CommandRunner, DiagnosticCode, Ecosystem, PackageId, ProjectRoot};

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

/// A Rust workspace living in a subdirectory leaves `version.workspace = true` members with no inheritance to resolve
/// against. Their publish targets and dependency edges are unknown, so the workspace is not complete.
#[test]
fn a_member_whose_inheritance_cannot_be_resolved_makes_the_workspace_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "native/Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n[workspace.package]\nversion = \"1.0.0\"\n",
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

    // `status` may still look at the partial workspace, with the manifest reported as an error.
    let partial = Workspace::scan(root.to_path_buf(), &locator, &NoopRunner).unwrap();
    assert!(partial
        .graph
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == DiagnosticCode::ManifestUnresolved));
    assert!(partial.require_complete().is_err());
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
