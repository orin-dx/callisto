//! Regression tests for two ecosystem-related bugs:
//!
//! Bug 1 (Critical): Python workspaces crash at graph load time because
//!   `IdentityResolver::resolve` has no `Ecosystem::Pypi` arm and falls
//!   through to `_ => Err(GraphError::AmbiguousName { name: "unsupported ecosystem", ... })`.
//!
//! Bug 2 (High): Cross-ecosystem cascade edges are silently absent because
//!   `IdentityIndex::resolve_native` only looks up `(eco, name)` keyed by the
//!   DECLARING package's ecosystem, so npm->cargo edges are never created.

use std::fs;
use std::path::Path;

use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::DependencyResolver;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner, PackageId};

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &Path,
    ) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn git_init(root: &Path) {
    std::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .output()
        .expect("git init should run");
}

// ---------------------------------------------------------------------------
// Bug 1: Python workspace load
// ---------------------------------------------------------------------------

/// A workspace containing only a `pyproject.toml` (PEP 621 style) must load
/// without error. Before the fix, `IdentityResolver::resolve` fell through to
/// `_ => Err(GraphError::AmbiguousName { name: "unsupported ecosystem", ... })`
/// for `Ecosystem::Pypi`, crashing `ManifestWalkResolver::build`.
#[test]
fn python_workspace_loads_without_crashing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init(root);

    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"my-python-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;

    let ws = Workspace::load(root.to_path_buf(), &locator, &runner);
    assert!(
        ws.is_ok(),
        "workspace load should succeed for python projects, got: {:?}",
        ws.err()
    );

    let ws = ws.unwrap();
    let pkgs: Vec<_> = ws.graph.packages().collect();
    assert_eq!(pkgs.len(), 1, "should discover exactly one package");
    assert_eq!(
        pkgs[0].id.name(),
        "my-python-pkg",
        "discovered package should have the name from pyproject.toml"
    );
}

/// Poetry-style `pyproject.toml` (name under `[tool.poetry]`) should also load.
#[test]
fn python_workspace_poetry_style_loads_without_crashing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init(root);

    fs::write(
        root.join("pyproject.toml"),
        "[tool.poetry]\nname = \"my-poetry-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;

    let ws = Workspace::load(root.to_path_buf(), &locator, &runner);
    assert!(
        ws.is_ok(),
        "workspace load should succeed for poetry-style python projects, got: {:?}",
        ws.err()
    );

    let ws = ws.unwrap();
    let pkgs: Vec<_> = ws.graph.packages().collect();
    assert_eq!(pkgs.len(), 1, "should discover exactly one package");
    assert_eq!(pkgs[0].id.name(), "my-poetry-pkg");
}

/// Mixed workspace: cargo crate + python package must both load.
#[test]
fn mixed_cargo_python_workspace_loads_without_crashing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init(root);

    // Workspace Cargo.toml
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    // Cargo crate
    let crate_dir = root.join("crates/my-lib");
    fs::create_dir_all(&crate_dir).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"my-lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // Python package alongside
    let py_dir = root.join("py-pkg");
    fs::create_dir_all(&py_dir).unwrap();
    fs::write(
        py_dir.join("pyproject.toml"),
        "[project]\nname = \"my-python-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;

    let ws = Workspace::load(root.to_path_buf(), &locator, &runner);
    assert!(
        ws.is_ok(),
        "mixed cargo+python workspace should load, got: {:?}",
        ws.err()
    );

    let ws = ws.unwrap();
    let pkg_names: Vec<_> = ws
        .graph
        .packages()
        .map(|p| p.id.name().to_string())
        .collect();
    assert_eq!(
        pkg_names.len(),
        2,
        "should discover 2 packages, got: {:?}",
        pkg_names
    );
    assert!(pkg_names.contains(&"my-lib".to_string()));
    assert!(pkg_names.contains(&"my-python-pkg".to_string()));
}

// ---------------------------------------------------------------------------
// Bug 2: Cross-ecosystem dep edge discovery
// ---------------------------------------------------------------------------

/// AC-12/AC-14: An npm package depending on a cargo crate's bare name must NOT
/// silently resolve across the ecosystem boundary. `resolve_native_with_fallback`'s
/// single-candidate cross-ecosystem match is suppressed (see T13a in
/// SPEC-TRACK3B1-IDENTITY-PROMOTION-CORE): no DepEdge is created from the npm
/// package to the cargo crate, and an UnknownPackage Warning diagnostic naming
/// `cargo:rust-addon` is recorded instead, so the unresolved dependency name is
/// visible rather than silently satisfied.
#[test]
fn cross_ecosystem_npm_to_cargo_dep_edge_is_discovered() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init(root);

    // Workspace Cargo.toml
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    // Cargo crate
    let crate_dir = root.join("crates/rust-addon");
    fs::create_dir_all(&crate_dir).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"rust-addon\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // npm package depending on the cargo crate by bare name
    let npm_dir = root.join("packages/my-app");
    fs::create_dir_all(&npm_dir).unwrap();
    fs::write(
        npm_dir.join("package.json"),
        r#"{"name":"my-app","version":"1.0.0","dependencies":{"rust-addon":"^1.0.0"}}"#,
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;

    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load");

    let cargo_id = PackageId::parse("rust-addon").unwrap();
    let npm_id = PackageId::parse("my-app").unwrap();

    // Both packages must be discovered
    let pkg_names: Vec<_> = ws
        .graph
        .packages()
        .map(|p| p.id.name().to_string())
        .collect();
    assert!(
        pkg_names.contains(&"rust-addon".to_string()),
        "cargo crate should be discovered; got: {:?}",
        pkg_names
    );
    assert!(
        pkg_names.contains(&"my-app".to_string()),
        "npm package should be discovered; got: {:?}",
        pkg_names
    );

    // No cross-ecosystem DepEdge is created (AC-12/AC-14): same-ecosystem-first
    // resolution treats the cargo-only name as unresolved from npm.
    let edges: Vec<_> = ws.graph.dependencies_of(&npm_id).collect();
    assert!(
        !edges.iter().any(|e| e.to == cargo_id),
        "cross-ecosystem edge my-app -> rust-addon must NOT exist (AC-12/AC-14); \
         found edges from my-app: {:?}",
        edges
            .iter()
            .map(|e| e.to.display_name())
            .collect::<Vec<_>>()
    );

    // An UnknownPackage Warning diagnostic naming cargo:rust-addon must be present instead.
    assert!(
        ws.graph.diagnostics().iter().any(|d| {
            d.code == callisto_model::DiagnosticCode::UnknownPackage
                && d.severity == callisto_model::DiagnosticSeverity::Warning
                && d.message.contains("cargo:rust-addon")
        }),
        "an UnknownPackage warning diagnostic naming cargo:rust-addon must be present"
    );
}

/// Unit test for `IdentityIndex::resolve_native` cross-ecosystem fallback.
/// Looking up a cargo crate by its bare name under the Npm ecosystem key
/// should return the crate when there is no ambiguity.
#[test]
fn resolve_native_falls_back_across_ecosystem_boundary() {
    use callisto_graph::IdentityIndex;
    use callisto_model::Ecosystem;

    let cargo_id = PackageId::parse("rust-addon").unwrap();

    let mut index = IdentityIndex::default();
    index.native.insert(
        (Ecosystem::Cargo, "rust-addon".to_string()),
        cargo_id.clone(),
    );

    // Same-ecosystem lookup still works
    assert_eq!(
        index.resolve_native(Ecosystem::Cargo, "rust-addon"),
        Some(&cargo_id),
        "same-ecosystem lookup should still work"
    );

    // Cross-ecosystem fallback: Npm lookup should find the Cargo crate
    assert_eq!(
        index.resolve_native(Ecosystem::Npm, "rust-addon"),
        Some(&cargo_id),
        "cross-ecosystem fallback should return the cargo crate when no npm entry exists"
    );
}

/// When the same bare name is indexed under two different ecosystems, and both
/// entries resolve to the same `PackageId` value, the cross-ecosystem fallback
/// correctly deduplicates and returns that single shared ID.
///
/// Note: in a valid workspace, two packages at *different* paths with the same
/// bare name would both produce `PackageId::Bare("name")` (identical values).
/// The walk builder already conflates them into a single `Package` entry, so
/// at the `resolve_native` level the two ecosystem registrations are not
/// truly ambiguous — they both point to the same logical package.
#[test]
fn resolve_native_deduplicates_same_bare_name_across_ecosystems() {
    use callisto_graph::IdentityIndex;
    use callisto_model::Ecosystem;

    // Both produce PackageId::Bare("shared-name") — equal values.
    let shared_id = PackageId::parse("shared-name").unwrap();

    let mut index = IdentityIndex::default();
    index.native.insert(
        (Ecosystem::Cargo, "shared-name".to_string()),
        shared_id.clone(),
    );
    index.native.insert(
        (Ecosystem::Pypi, "shared-name".to_string()),
        shared_id.clone(),
    );

    // Npm lookup: both Cargo and Pypi entries resolve to the same PackageId,
    // so dedup collapses them and returns that single ID.
    let result = index.resolve_native(Ecosystem::Npm, "shared-name");
    assert_eq!(
        result,
        Some(&shared_id),
        "two ecosystem entries with equal PackageIds should deduplicate to one result"
    );
}
