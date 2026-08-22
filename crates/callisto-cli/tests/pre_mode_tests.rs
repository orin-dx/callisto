mod common;

use std::fs;
use std::process::Command;

use callisto_cli::cli::{AddArgs, GlobalArgs, InitArgs, OutputFormat, PreArgs, VersionArgs};
use callisto_cli::commands;

use common::setup_polyglot_git_repo;

#[test]
fn test_pre_mode_blackbox_lifecycle() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    // 1. Enter pre-release mode with tag 'beta'
    let enter_res = commands::pre::handle(
        PreArgs::Enter {
            tag: "beta".to_string(),
        },
        &global,
    );
    assert!(enter_res.is_ok());
    assert!(root.join(".changeset/pre.json").exists());

    // 2. Add changeset for minor bump
    let add_res = commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:minor".to_string()],
            summary: Some("Beta feature".to_string()),
        },
        &global,
    );
    assert!(add_res.is_ok());

    // 3. Version in pre-release mode -> should produce 0.2.0-beta.0
    let version_res = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            strict_graph: false,
            allow_empty_changesets: false,
        },
        &global,
    );
    assert!(version_res.is_ok());

    let updated_cargo = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(updated_cargo.contains("0.2.0-beta.0"));

    // 4. Exit pre-release mode
    let exit_res = commands::pre::handle(PreArgs::Exit, &global);
    assert!(exit_res.is_ok());

    // 5. Final versioning after exit -> should finalize to 0.2.0
    let final_version_res = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        },
        &global,
    );
    assert!(final_version_res.is_ok());

    let final_cargo = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(
        final_cargo.contains("version = \"0.2.0\""),
        "Expected final version 0.2.0 in Cargo.toml, got:\n{final_cargo}"
    );
}

/// Regression test: if `.changeset/pre.json` exists but is malformed (truncated,
/// corrupted, etc.), `callisto version` must return `Err`, not silently fall back
/// to normal release mode and consume changesets as if no pre-release was active.
///
/// Previously, `.ok()` swallowed all parse errors and returned `None`, causing
/// the version command to behave as if no pre.json existed at all.
#[test]
fn test_malformed_pre_json_returns_error_not_silent_normal_release() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::init::handle(InitArgs { yes: true }, &global).unwrap();

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("Test patch".to_string()),
        },
        &global,
    )
    .unwrap();

    let changeset_dir = root.join(".changeset");
    fs::write(changeset_dir.join("pre.json"), r#"{"mode": "p"#).unwrap();

    let version_res = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            strict_graph: false,
            allow_empty_changesets: false,
        },
        &global,
    );
    assert!(
        version_res.is_err(),
        "expected Err when .changeset/pre.json is malformed, but version command returned Ok"
    );

    let cargo_content = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(
        cargo_content.contains("version = \"0.1.0\""),
        "Cargo.toml must not be modified when pre.json is malformed, but got:\n{cargo_content}"
    );
}

/// Bug 1: `pre enter` must refuse to overwrite an existing pre.json.
///
/// Running `pre enter` a second time while the workspace is already in
/// pre-release mode should return an error, not silently overwrite the file.
/// The check must fire in both the real-write and dry-run paths.
#[test]
fn test_pre_enter_rejects_if_already_in_pre_mode() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    let first = commands::pre::handle(
        PreArgs::Enter {
            tag: "alpha".to_string(),
        },
        &global,
    );
    assert!(first.is_ok(), "first pre enter must succeed: {first:?}");
    assert!(
        root.join(".changeset/pre.json").exists(),
        "pre.json must exist after first enter"
    );

    let second = commands::pre::handle(
        PreArgs::Enter {
            tag: "beta".to_string(),
        },
        &global,
    );
    assert!(
        second.is_err(),
        "second pre enter must fail when pre.json already exists, but got Ok"
    );

    let content = fs::read_to_string(root.join(".changeset/pre.json")).unwrap();
    assert!(
        content.contains("alpha"),
        "pre.json must still contain 'alpha' after rejected second enter: {content}"
    );
    assert!(
        !content.contains("beta"),
        "pre.json must NOT contain 'beta' after rejected second enter: {content}"
    );

    let global_dry = GlobalArgs {
        dry_run: true,
        ..global.clone()
    };
    let dry = commands::pre::handle(PreArgs::Enter { tag: "rc".to_string() }, &global_dry);
    assert!(
        dry.is_err(),
        "dry-run pre enter must also fail when pre.json already exists on disk"
    );
}

/// Bug 2: `pre exit` must refuse to run when already in Exit mode (double-exit).
#[test]
fn test_pre_exit_rejects_double_exit() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::pre::handle(
        PreArgs::Enter {
            tag: "alpha".to_string(),
        },
        &global,
    )
    .expect("pre enter must succeed");

    let first_exit = commands::pre::handle(PreArgs::Exit, &global);
    assert!(first_exit.is_ok(), "first pre exit must succeed: {first_exit:?}");

    let second_exit = commands::pre::handle(PreArgs::Exit, &global);
    assert!(
        second_exit.is_err(),
        "second pre exit must fail when already in Exit mode, but got Ok"
    );
}

/// Bug 3: `pre enter` must reject an empty tag string.
///
/// An empty tag produces `"1.0.0-.0"` during versioning, which is not valid semver.
#[test]
fn test_pre_enter_rejects_empty_tag() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    let result = commands::pre::handle(PreArgs::Enter { tag: "".to_string() }, &global);
    assert!(
        result.is_err(),
        "pre enter with empty tag must return an error, but got Ok"
    );

    assert!(
        !root.join(".changeset/pre.json").exists(),
        "pre.json must not be created when the tag is empty"
    );
}

/// Bug 4: `pre enter` must stage `.changeset/pre.json` via `git add`.
///
/// After a successful `pre enter`, `git status --porcelain` should show
/// `.changeset/pre.json` as staged (index status 'A'), not as untracked ('??').
#[test]
fn test_pre_enter_stages_pre_json_via_git_add() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::pre::handle(
        PreArgs::Enter {
            tag: "alpha".to_string(),
        },
        &global,
    )
    .expect("pre enter must succeed");

    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .expect("git status must run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    let is_staged = stdout
        .lines()
        .any(|line| (line.starts_with('A') || line.starts_with("AM")) && line.contains("pre.json"));
    let is_untracked = stdout.lines().any(|l| l.starts_with("??") && l.contains("pre.json"));

    assert!(
        is_staged,
        "pre.json must be staged (git add) after pre enter, but git status shows:\n{stdout}"
    );
    assert!(
        !is_untracked,
        "pre.json must NOT be untracked after pre enter, but git status shows:\n{stdout}"
    );
}
