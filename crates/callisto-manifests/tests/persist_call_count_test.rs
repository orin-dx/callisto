//! Regression tests for callisto_manifests::persist_call_count/reset_persist_call_count.
//! Isolated in its own integration-test binary because PERSIST_CALL_COUNT is a
//! process-global counter that other, non-#[serial] tests in the `--lib` binary
//! (spread across cargo.rs's/npm.rs's/python.rs's own test modules) would
//! pollute -- see crates/callisto-graph/tests/apply_persist_open_count_test.rs's
//! identical documented precedent for OPEN_CALL_COUNT.

use std::fs;

use callisto_manifests::{open, OpenContext, WorkspaceCargoResolver};
use callisto_model::{ApplyPermit, ManifestDecl, ManifestFormat, ManifestRole, Version, VersionGrammar};
use serial_test::serial;

fn permit() -> ApplyPermit {
    ApplyPermit::force_for_tests()
}

#[test]
#[serial]
fn cargo_persist_increments_persist_call_count_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    let mut manifest = open(&decl, &ctx).unwrap();

    callisto_manifests::reset_persist_call_count();
    assert_eq!(
        callisto_manifests::persist_call_count(),
        0,
        "reset_persist_call_count() must genuinely zero the counter, not merely be a no-op alongside a hardcoded getter"
    );
    manifest.persist(&permit()).unwrap();

    assert_eq!(
        callisto_manifests::persist_call_count(),
        1,
        "a successful persist() must increment PERSIST_CALL_COUNT by exactly 1"
    );
}

#[test]
#[serial]
fn npm_persist_increments_persist_call_count_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("package.json");
    let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\"\n}\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    let mut manifest = open(&decl, &ctx).unwrap();

    callisto_manifests::reset_persist_call_count();
    manifest.persist(&permit()).unwrap();

    assert_eq!(
        callisto_manifests::persist_call_count(),
        1,
        "a successful persist() must increment PERSIST_CALL_COUNT by exactly 1"
    );
}

#[test]
#[serial]
fn python_persist_increments_persist_call_count_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("pyproject.toml");
    let content = "[project]\nname = \"my-python-lib\"\nversion = \"0.3.1\"\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("pyproject.toml", ManifestRole::Canonical, ManifestFormat::PyprojectToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    let mut manifest = open(&decl, &ctx).unwrap();

    callisto_manifests::reset_persist_call_count();
    manifest.persist(&permit()).unwrap();

    assert_eq!(
        callisto_manifests::persist_call_count(),
        1,
        "a successful persist() must increment PERSIST_CALL_COUNT by exactly 1"
    );
}

/// `WorkspaceCargoResolver::load`/`persist` are tracked by their own
/// `resolver_load_call_count`/`resolver_persist_call_count` counters, kept
/// separate from `open_call_count`/`persist_call_count` (see those
/// counters' doc comments in `lib.rs` for why: `OpenContext::for_workspace_root`
/// calls `WorkspaceCargoResolver::load` unconditionally whenever a
/// workspace root `Cargo.toml` exists, which would otherwise pollute
/// `open_call_count()` assertions workspace-wide).
#[test]
#[serial]
fn workspace_cargo_resolver_load_and_persist_increment_their_own_counters() {
    let dir = tempfile::tempdir().unwrap();
    let root_cargo_path = dir.path().join("Cargo.toml");
    fs::write(
        &root_cargo_path,
        "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    callisto_manifests::reset_resolver_load_call_count();
    callisto_manifests::reset_resolver_persist_call_count();
    assert_eq!(callisto_manifests::resolver_load_call_count(), 0);
    assert_eq!(callisto_manifests::resolver_persist_call_count(), 0);

    let mut resolver = WorkspaceCargoResolver::load(&root_cargo_path).unwrap();
    assert_eq!(
        callisto_manifests::resolver_load_call_count(),
        1,
        "WorkspaceCargoResolver::load must increment resolver_load_call_count by exactly 1"
    );
    assert_eq!(
        callisto_manifests::resolver_persist_call_count(),
        0,
        "load() alone must not touch resolver_persist_call_count"
    );

    let new_ver = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();
    resolver.write_version(&new_ver, &permit()).unwrap();
    assert_eq!(
        callisto_manifests::resolver_persist_call_count(),
        0,
        "write_version() alone (no persist()) must not increment resolver_persist_call_count"
    );

    resolver.persist(&permit()).unwrap();
    assert_eq!(
        callisto_manifests::resolver_persist_call_count(),
        1,
        "a successful persist() must increment resolver_persist_call_count by exactly 1"
    );
}
