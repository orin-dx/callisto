//! `callisto init`'s reconcile behavior (docs/00-design.md §18 Q5.4 mechanism 1,
//! docs/01-spec.md §18 Q5.5, §G.11): re-running `init` on an already-initialized
//! workspace re-detects the current workspace state, computes a diff against
//! what is already recorded in `callisto.toml`, and applies changes only with
//! confirmation (`opts.yes` standing in for that confirmation in non-interactive
//! use). Between confirmed runs, drift must never be silently written.

use std::fs;
use std::path::Path;

use callisto_graph::commands::{init, InitOptions};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner, Ecosystem};

/// Every test here exercises a real (non-dry) init, so each hands `init` a
/// permit. The dry-run counterparts live in callisto-cli's
/// `dry_run_invariant_tests`, which assert the whole tree is untouched.
fn permit() -> callisto_model::ApplyPermit {
    callisto_model::ApplyPermit::force_for_tests()
}

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

/// A Cargo-only workspace: `Cargo.toml` + one member crate.
fn write_cargo_workspace(root: &Path) {
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

    let crate_dir = root.join("crates/engine");
    fs::create_dir_all(&crate_dir).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"engine\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
}

/// Adds an npm package to a workspace that previously had none, simulating a
/// workspace that "gained a new ecosystem" between `init` runs.
fn add_npm_package(root: &Path) {
    let pkg_dir = root.join("packages/web");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("package.json"),
        "{\n  \"name\": \"@myorg/web\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();
}

fn load_workspace<'a>(root: &Path, runner: &'a NoopRunner) -> Workspace<'a, NoopRunner> {
    let locator = IgnoreWalkLocator::new(root);
    Workspace::load(root.to_path_buf(), &locator, runner).expect("workspace should load")
}

/// First run on a fresh Cargo-only workspace: `callisto.toml` and
/// `.changeset/` are scaffolded, and there is nothing to reconcile.
#[test]
fn first_run_on_fresh_workspace_writes_config_with_no_drift() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_cargo_workspace(root);

    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);

    let report =
        init(&ws, &InitOptions { yes: false }, Some(&permit())).expect("init should succeed");

    assert!(
        report.initialized,
        "first run must report initialized = true"
    );
    assert!(root.join("callisto.toml").exists());
    assert!(root.join(".changeset").is_dir());
    assert!(
        report.diff.new_ecosystems.is_empty(),
        "a first run has nothing pre-existing to diff against, so there is no drift to report: {:?}",
        report.diff.new_ecosystems
    );
    assert!(
        !report.diff.applied,
        "first run is a direct write, not a reconcile-apply"
    );
}

/// Re-running `init` on a workspace that already has `callisto.toml` but has
/// gained a new ecosystem (a `package.json` added to a previously Cargo-only
/// workspace) with `yes: false` must detect and report the drift without
/// touching any files on disk.
#[test]
fn rerun_with_yes_false_reports_new_ecosystem_without_mutating_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_cargo_workspace(root);

    let runner = NoopRunner;

    // First run: establishes the recorded baseline (cargo only).
    let first_ws = load_workspace(root, &runner);
    let first_report = init(&first_ws, &InitOptions { yes: true }, Some(&permit()))
        .expect("first init should succeed");
    assert!(first_report.initialized);

    let config_before = fs::read_to_string(root.join("callisto.toml")).unwrap();
    let changeset_readme_before = fs::read_to_string(root.join(".changeset/README.md")).unwrap();

    // The workspace gains npm.
    add_npm_package(root);

    let second_ws = load_workspace(root, &runner);
    let report = init(&second_ws, &InitOptions { yes: false }, Some(&permit()))
        .expect("reconcile init should succeed");

    assert_eq!(
        report.diff.new_ecosystems,
        vec![Ecosystem::Npm],
        "npm's appearance must be detected as drift: {:?}",
        report.diff.new_ecosystems
    );
    assert!(
        !report.diff.applied,
        "yes:false must not apply the detected diff"
    );

    let config_after = fs::read_to_string(root.join("callisto.toml")).unwrap();
    assert_eq!(
        config_before, config_after,
        "callisto.toml must be byte-identical after a yes:false dry-preview run"
    );
    let changeset_readme_after = fs::read_to_string(root.join(".changeset/README.md")).unwrap();
    assert_eq!(
        changeset_readme_before, changeset_readme_after,
        ".changeset/README.md must not be touched by a yes:false dry-preview run"
    );
}

/// Same drift scenario as above, but with `yes: true`: the diff must be
/// applied and `callisto.toml` updated to reflect the new ecosystem.
#[test]
fn rerun_with_yes_true_applies_new_ecosystem_to_config() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_cargo_workspace(root);

    let runner = NoopRunner;

    let first_ws = load_workspace(root, &runner);
    init(&first_ws, &InitOptions { yes: true }, Some(&permit()))
        .expect("first init should succeed");

    add_npm_package(root);

    let second_ws = load_workspace(root, &runner);
    let report = init(&second_ws, &InitOptions { yes: true }, Some(&permit()))
        .expect("reconcile init should succeed");

    assert_eq!(report.diff.new_ecosystems, vec![Ecosystem::Npm]);
    assert!(report.diff.applied, "yes:true must apply the detected diff");

    // A third run, with no further drift, must now see nothing to reconcile —
    // proof that the apply was actually persisted to callisto.toml.
    let third_ws = load_workspace(root, &runner);
    let idempotent_report = init(&third_ws, &InitOptions { yes: false }, Some(&permit()))
        .expect("idempotent init should succeed");
    assert!(
        idempotent_report.diff.new_ecosystems.is_empty(),
        "after an applied reconcile, re-running init must be a no-op: {:?}",
        idempotent_report.diff.new_ecosystems
    );
    assert!(!idempotent_report.diff.applied);
}
