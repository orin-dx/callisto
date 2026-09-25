/// Tests for the `--strict` flag on `callisto snapshot`.
///
/// The command must abort with a `CliError` when `--strict` is passed and
/// the workspace graph contains diagnostics escalated to
/// `DiagnosticSeverity::Error` under strict mode. The escalation unit tests in
/// `callisto-cli/src/commands/mod.rs` prove the abort path; these tests prove a
/// clean graph does not abort.
///
use std::fs;
use std::process::ExitCode;
use tempfile::TempDir;

use callisto_cli::cli::{GlobalArgs, OutputFormat, SnapshotArgs, VersionArgs};
use callisto_cli::commands;

fn make_git_workspace(tmp: &TempDir) -> GlobalArgs {
    let root = tmp.path();

    callisto_fixtures::git::init_repo(root);

    // Minimal Cargo workspace with one package.
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/pkg-a\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    let pkg = root.join("crates/pkg-a");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: true, // dry-run so no disk writes needed for git ops
    };

    // Initialize callisto.toml.
    callisto_fixtures::scaffold_callisto(&global.cwd);

    // git add + commit so HEAD exists (snapshot needs HEAD SHA).
    drop(
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(root)
            .output(),
    );
    drop(
        std::process::Command::new("git")
            .args(["commit", "-m", "initial", "--allow-empty"])
            .current_dir(root)
            .output(),
    );

    global
}

// ---------------------------------------------------------------------------
// Version --strict tests
// ---------------------------------------------------------------------------

/// Regression test for Bug 2: `callisto version --strict` must exit non-zero
/// when strict violations produce Error-severity diagnostics.
///
/// A workspace with no pending changesets emits an `EmptyChangeset` warning that
/// `--strict` escalates to `Error`. Before the fix, the CLI handler ignored the
/// escalated diagnostics and always returned `ExitCode::SUCCESS`. After the fix,
/// it gates on `Error`-severity diagnostics and returns `ExitCode::FAILURE`.
#[test]
fn test_version_strict_no_changesets_exits_nonzero() {
    let tmp = TempDir::new().unwrap();
    // dry_run=true so no manifest writes happen; the version plan is still computed.
    let global = make_git_workspace(&tmp);

    let args = VersionArgs {
        strict: true,
        allow_empty_changesets: false,
        refresh_lockfiles: false,
        emit_decision: None,
    };

    let result = commands::version::handle(args, &global);
    assert!(
        result.is_ok(),
        "version --strict should return Ok(ExitCode); got Err: {result:?}"
    );
    let code = result.unwrap();
    assert_ne!(
        format!("{code:?}"),
        format!("{:?}", ExitCode::SUCCESS),
        "version --strict with no changesets must exit non-zero (EmptyChangeset escalated to Error)"
    );
}

/// `callisto version` without `--strict` on an empty workspace exits 0.
/// The EmptyChangeset warning is not escalated to Error so no failure fires.
#[test]
fn test_version_no_strict_no_changesets_succeeds() {
    let tmp = TempDir::new().unwrap();
    let global = make_git_workspace(&tmp);

    let args = VersionArgs {
        strict: false,
        allow_empty_changesets: false,
        refresh_lockfiles: false,
        emit_decision: None,
    };

    let result = commands::version::handle(args, &global);
    assert!(
        result.is_ok(),
        "version without --strict should succeed; got: {result:?}"
    );
    let code = result.unwrap();
    assert_eq!(
        format!("{code:?}"),
        format!("{:?}", ExitCode::SUCCESS),
        "version without --strict on empty workspace should exit 0"
    );
}

// ---------------------------------------------------------------------------
// Snapshot --strict tests
// ---------------------------------------------------------------------------

/// `callisto snapshot --strict` on a workspace with no error diagnostics
/// must succeed (return Ok with ExitCode::SUCCESS).
///
/// A clean graph has no `Error`-severity diagnostics even after escalation, so
/// the abort path must not trigger.
#[test]
fn test_snapshot_strict_clean_graph_succeeds() {
    let tmp = TempDir::new().unwrap();
    let global = make_git_workspace(&tmp);

    let args = SnapshotArgs {
        tag: "ci".to_string(),
        strict: true,
    };

    let result = commands::snapshot::handle(args, &global);
    assert!(
        result.is_ok(),
        "snapshot --strict on a clean graph should succeed; got: {result:?}"
    );
    let code = result.unwrap();
    assert_eq!(
        format!("{code:?}"),
        format!("{:?}", ExitCode::SUCCESS),
        "snapshot --strict on a clean graph should return exit code 0"
    );
}

/// `callisto snapshot` without `--strict` on a clean workspace must also
/// succeed, confirming no regression.
#[test]
fn test_snapshot_no_strict_clean_graph_succeeds() {
    let tmp = TempDir::new().unwrap();
    let global = make_git_workspace(&tmp);

    let args = SnapshotArgs {
        tag: "ci".to_string(),
        strict: false,
    };

    let result = commands::snapshot::handle(args, &global);
    assert!(
        result.is_ok(),
        "snapshot without --strict should succeed; got: {result:?}"
    );
}
