//! `status --check`'s well-formedness diagnostics -- covers what `validate`
//! (removed, SPEC-DX-STATUS-ADD AC-02/AC-03) used to check.

mod common;

use std::fs;
use std::process::{Command, ExitCode};

use callisto_cli::cli::{AddArgs, GlobalArgs, OutputFormat, StatusArgs};
use callisto_cli::commands;

use common::setup_polyglot_git_repo;

/// A git repository where a Cargo crate and an npm package share the same
/// bare name (`foo`), so a changeset entry naming bare `foo` is ambiguous
/// between them.
fn setup_ambiguous_name_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Callisto Tester"],
        vec!["config", "user.email", "tester@callisto.dev"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "tag.gpgsign", "false"],
    ] {
        assert!(Command::new("git")
            .args(&args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/foo/src")).unwrap();
    fs::write(
        root.join("crates/foo/Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/foo/src/lib.rs"), "pub fn hello() {}\n").unwrap();

    fs::create_dir_all(root.join("packages/foo")).unwrap();
    fs::write(
        root.join("packages/foo/package.json"),
        "{\n  \"name\": \"foo\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();

    assert!(Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());

    dir
}

fn global(root: &std::path::Path) -> GlobalArgs {
    GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    }
}

fn check_args() -> StatusArgs {
    StatusArgs {
        strict: false,
        check: true,
    }
}

#[test]
fn test_status_check_clean_workspace_exits_ok() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let global = global(root);

    callisto_fixtures::scaffold_callisto(&global.cwd);

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("A valid changeset".to_string()),
        },
        &global,
    )
    .unwrap();

    // A pending changeset with no diagnostic errors: exit code 0, not FAILURE
    // -- SPEC-DX-STATUS-ADD AC-04 made pending state exit-code-irrelevant.
    let result = commands::status::handle(check_args(), &global);
    assert_eq!(
        result.unwrap(),
        ExitCode::SUCCESS,
        "status --check must succeed when all pending changesets are well-formed"
    );
}

#[test]
fn test_status_check_malformed_changeset_exits_failure() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let global = global(root);

    callisto_fixtures::scaffold_callisto(&global.cwd);

    // A changeset naming an unknown package -- an Error-severity diagnostic
    // `validate` used to catch as `UnknownPackage`.
    let changeset_dir = root.join(".changeset");
    fs::write(
        changeset_dir.join("bad-changeset.md"),
        "---\ncargo/does-not-exist: patch\n---\n\nSome change.\n",
    )
    .unwrap();

    let result = commands::status::handle(check_args(), &global);
    assert_eq!(
        result.unwrap(),
        ExitCode::FAILURE,
        "status --check must exit 1 (FAILURE) when a changeset names an unknown package"
    );
}

/// SPEC-DX-STATUS-ADD AC-03: a changeset naming a bare package that's
/// ambiguous between ecosystems must report the `AmbiguousPackageName`
/// diagnostic (exit 1), not hard-error out of `status` entirely.
#[test]
fn test_status_check_ambiguous_bare_name_reports_diagnostic_not_hard_error() {
    let dir = setup_ambiguous_name_git_repo();
    let root = dir.path();
    let global = global(root);

    callisto_fixtures::scaffold_callisto(&global.cwd);

    let changeset_dir = root.join(".changeset");
    fs::write(
        changeset_dir.join("ambiguous-changeset.md"),
        "---\nfoo: patch\n---\n\nSome change.\n",
    )
    .unwrap();

    let result = commands::status::handle(check_args(), &global);
    assert_eq!(
        result.unwrap(),
        ExitCode::FAILURE,
        "status --check must exit 1 (FAILURE) via the AmbiguousPackageName diagnostic, not hard-error"
    );
}

/// `callisto add --packages unknown-pkg:patch` must return an error when the
/// package name is not present in the workspace, rather than silently writing
/// a changeset that will fail during `callisto version`.
#[test]
fn test_add_unknown_package_name_returns_error() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let global = global(root);

    callisto_fixtures::scaffold_callisto(&global.cwd);

    // "completely-unknown-pkg" is not in the workspace (workspace has "core-crate" and "@myorg/web-app")
    let result = commands::add::handle(
        AddArgs {
            packages: vec!["completely-unknown-pkg:patch".to_string()],
            summary: Some("This package does not exist".to_string()),
        },
        &global,
    );

    assert!(
        result.is_err(),
        "add must fail when the specified package does not exist in the workspace"
    );
}
