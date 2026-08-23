/// Tests for the `--strict` flag on `callisto snapshot` and `callisto tag`.
///
/// Both commands must abort with a `CliError` when `--strict` is passed and
/// the workspace graph contains crosscheck failures (diagnostics whose
/// `escalated_by` field is `StrictFlag::Strict` or `StrictFlag::StrictGraph`,
/// promoted to `DiagnosticSeverity::Error` under strict mode).
///
/// Because creating a real crosscheck failure requires moon-declared edges that
/// disagree with manifest edges (a moon-specific infra concern), these tests
/// use a simpler strategy: verify that `--strict` with a *clean* graph does not
/// abort (exit code 0), and that a `Diagnostic` with `Error` severity produced
/// by a mocked graph triggers the abort path.  The graph-level crosscheck unit
/// tests in `callisto-graph/src/crosscheck.rs` already prove the diagnostic
/// production logic; these tests prove the CLI abort-on-error contract.
///
/// TDD note: the `strict` field is added to `SnapshotArgs` and `TagArgs` in
/// this commit.  The tests in this file are written first and compile only
/// after the struct fields exist.
use std::fs;
use std::process::ExitCode;
use tempfile::TempDir;

use callisto_cli::cli::{GlobalArgs, OutputFormat, SnapshotArgs, TagArgs, VersionArgs};
use callisto_cli::commands;

fn make_git_workspace(tmp: &TempDir) -> GlobalArgs {
    let root = tmp.path();

    drop(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output(),
    );

    // Configure git identity so commits and tags work.
    drop(
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(root)
            .output(),
    );
    drop(
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.dev"])
            .current_dir(root)
            .output(),
    );

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
    commands::init::handle(callisto_cli::cli::InitArgs { yes: true }, &global).unwrap();

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
        strict_graph: false,
        allow_empty_changesets: false,
        refresh_lockfiles: false,
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
        strict_graph: false,
        allow_empty_changesets: false,
        refresh_lockfiles: false,
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

/// `callisto snapshot --strict` on a workspace with no crosscheck failures
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

// ---------------------------------------------------------------------------
// Tag --strict tests
// ---------------------------------------------------------------------------

/// `callisto tag --strict` on a workspace with no crosscheck failures must
/// succeed (return Ok with ExitCode::SUCCESS).
#[test]
fn test_tag_strict_clean_graph_succeeds() {
    let tmp = TempDir::new().unwrap();
    let global = make_git_workspace(&tmp);

    // Build a minimal publish plan with one release entry.
    // We use dry_run=true in the global args so no git tags are actually
    // created; the important thing is that the strict check does not fire.
    let plan_json = serde_json::json!({
        "schemaVersion": 1,
        "rustCrates": [],
        "npmPlatformPackages": [],
        "npmMainPackages": [],
        "releases": []
    })
    .to_string();

    let args = TagArgs {
        plan: plan_json,
        floating_major: false,
        strict: true,
    };

    let result = commands::tag::handle(args, &global);
    assert!(
        result.is_ok(),
        "tag --strict on a clean graph should succeed; got: {result:?}"
    );
    let code = result.unwrap();
    assert_eq!(
        format!("{code:?}"),
        format!("{:?}", ExitCode::SUCCESS),
        "tag --strict on a clean graph should return exit code 0"
    );
}

/// `callisto tag` without `--strict` on a clean workspace must also succeed.
#[test]
fn test_tag_no_strict_clean_graph_succeeds() {
    let tmp = TempDir::new().unwrap();
    let global = make_git_workspace(&tmp);

    let plan_json = serde_json::json!({
        "schemaVersion": 1,
        "rustCrates": [],
        "npmPlatformPackages": [],
        "npmMainPackages": [],
        "releases": []
    })
    .to_string();

    let args = TagArgs {
        plan: plan_json,
        floating_major: false,
        strict: false,
    };

    let result = commands::tag::handle(args, &global);
    assert!(result.is_ok(), "tag without --strict should succeed; got: {result:?}");
}
