//! `status --check`'s well-formedness diagnostics -- covers what `validate`
//! (removed, SPEC-DX-STATUS-ADD AC-02/AC-03) used to check.

mod common;

use std::fs;
use std::process::ExitCode;

use callisto_cli::cli::{AddArgs, GlobalArgs, OutputFormat, StatusArgs};
use callisto_cli::commands;

use common::setup_polyglot_git_repo;

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
        strict_graph: false,
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
