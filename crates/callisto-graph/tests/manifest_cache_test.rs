//! Regression test for redundant manifest reads.
//!
//! A single canonical manifest (e.g. one package's `Cargo.toml`) must be
//! opened via `callisto_manifests::open` at most once per read-only command
//! run, even though three independent call sites consult it:
//!
//!   1. `ManifestWalkResolver::build` — publish_targets() scan
//!   2. `ManifestWalkResolver::build` — iter_dependencies() scan
//!   3. `Workspace::base_versions` — current_version() read
//!
//! Without memoization, each of the two packages built below is opened once
//! per site (6 opens total for 2 packages). With a shared read-only cache,
//! each package's manifest is opened exactly once (2 opens total).

use std::fs;
use std::path::Path;

use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner};
use serial_test::serial;

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

fn build_two_package_workspace(root: &Path) {
    std::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .output()
        .expect("git init should run");

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    let dep_dir = root.join("crates/dep-pkg");
    fs::create_dir_all(&dep_dir).unwrap();
    fs::write(
        dep_dir.join("Cargo.toml"),
        "[package]\nname = \"dep-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let app_dir = root.join("crates/app-pkg");
    fs::create_dir_all(&app_dir).unwrap();
    fs::write(
        app_dir.join("Cargo.toml"),
        "[package]\nname = \"app-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ndep-pkg = { path = \"../dep-pkg\", version = \"0.1.0\" }\n",
    )
    .unwrap();
}

#[test]
#[serial]
fn base_versions_reuses_manifests_opened_during_graph_discovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_two_package_workspace(root);

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;

    // This counter is process-global (see callisto_manifests::open_call_count).
    // This test must not run concurrently with other tests in this binary
    // that also open real manifests, so it is the sole `open()`-exercising
    // test in this file.
    callisto_manifests::reset_open_call_count();

    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load");

    let opens_after_discovery = callisto_manifests::open_call_count();
    assert_eq!(
        opens_after_discovery, 2,
        "graph discovery (publish_targets + iter_dependencies scans) opened each \
         of the 2 package manifests {opens_after_discovery} time(s) total; expected \
         exactly 1 open per manifest (2 total), not one open per scan"
    );

    let _base_versions = ws.base_versions().expect("base_versions should succeed");

    let opens_after_base_versions = callisto_manifests::open_call_count();
    assert_eq!(
        opens_after_base_versions, 2,
        "Workspace::base_versions() re-opened manifests that graph discovery had \
         already opened; total opens grew from {opens_after_discovery} to \
         {opens_after_base_versions}, expected it to stay at 2 (fully reused from cache)"
    );
}
