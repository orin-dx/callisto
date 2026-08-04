mod common;

use std::fs;
use std::process::ExitCode;

use callisto_cli::cli::{AddArgs, GlobalArgs, InitArgs, OutputFormat, ValidateArgs};
use callisto_cli::commands;

use common::setup_polyglot_git_repo;

#[test]
fn test_validate_clean_workspace_exits_zero() {
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
            summary: Some("A valid changeset".to_string()),
        },
        &global,
    )
    .unwrap();

    let result = commands::validate::handle(
        ValidateArgs {
            staged: false,
            since: None,
            strict: false,
            strict_graph: false,
        },
        &global,
    );
    assert_eq!(
        result.unwrap(),
        std::process::ExitCode::SUCCESS,
        "validate must exit 0 when all changesets are valid"
    );
}

#[test]
fn test_validate_malformed_changeset_exits_nonzero() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::init::handle(InitArgs { yes: true }, &global).unwrap();

    // Write a changeset with an empty summary body (invalid — entries present but no text)
    let changeset_dir = root.join(".changeset");
    fs::write(
        changeset_dir.join("bad-changeset.md"),
        "---\ncargo/core-crate: patch\n---\n\n",
    )
    .unwrap();

    let result = commands::validate::handle(
        ValidateArgs {
            staged: false,
            since: None,
            strict: false,
            strict_graph: false,
        },
        &global,
    );

    // validate may return Ok(ExitCode::FAILURE) or Err — either signals a non-clean workspace.
    // Here it returns Ok(FAILURE) because validate reports it as a validation error, not a crash.
    if let Ok(code) = result {
        assert_ne!(
            code,
            ExitCode::SUCCESS,
            "validate must not exit 0 when a changeset has an empty summary"
        );
    }
}

/// `callisto add --packages unknown-pkg:patch` must return an error when the
/// package name is not present in the workspace, rather than silently writing
/// a changeset that will fail during `callisto version`.
///
/// Currently this is a known gap: the non-interactive add path only validates
/// the format of the package name via `PackageId::parse`, not whether the
/// package actually exists in `ws.graph.packages()`. When the production fix
/// lands, this test documents the expected error behavior.
#[test]
fn test_add_unknown_package_name_returns_error() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::init::handle(InitArgs { yes: true }, &global).unwrap();

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
        "add must fail when the specified package does not exist in the workspace, \
         but it returned Ok — no validation against workspace packages is happening"
    );
}
