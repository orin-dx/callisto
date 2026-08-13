//! Regression tests for callisto_manifests::persist_call_count/reset_persist_call_count.
//! Isolated in its own integration-test binary because PERSIST_CALL_COUNT is a
//! process-global counter that other, non-#[serial] tests in the `--lib` binary
//! (spread across cargo.rs's/npm.rs's/python.rs's own test modules) would
//! pollute -- see crates/callisto-graph/tests/apply_persist_open_count_test.rs's
//! identical documented precedent for OPEN_CALL_COUNT.

use std::fs;

use callisto_manifests::{open, OpenContext};
use callisto_model::{ApplyPermit, ManifestDecl, ManifestFormat, ManifestRole};
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

    let decl = ManifestDecl::new(
        "Cargo.toml",
        ManifestRole::Canonical,
        ManifestFormat::CargoToml,
    )
    .unwrap();
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
fn npm_persist_increments_persist_call_count_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("package.json");
    let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\"\n}\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new(
        "package.json",
        ManifestRole::Canonical,
        ManifestFormat::PackageJson,
    )
    .unwrap();
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

    let decl = ManifestDecl::new(
        "pyproject.toml",
        ManifestRole::Canonical,
        ManifestFormat::PyprojectToml,
    )
    .unwrap();
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
