//! Coverage for two things that only manifest through a real subprocess
//! invocation of the compiled binary:
//!
//! 1. `main.rs`'s per-subcommand dispatch arms (`Command::X(args) =>
//!    x::handle(...)`) -- these live in the binary crate, not the library, so
//!    they are only exercised by spawning `callisto` itself, never by calling
//!    `commands::x::handle()` in-process.
//! 2. The `--plan -` / `--existing-body -` "read from stdin" branches on
//!    `tag` and `compose-pr-body` -- `std::io::stdin()` reads the real
//!    process stdin, which an in-process unit test cannot redirect.

mod common;

use std::io::Write;
use std::process::{Command, Stdio};

use common::setup_polyglot_git_repo;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_callisto")
}

/// Drives one full lifecycle through the real binary so every subcommand's
/// `main.rs` dispatch arm actually executes: init, status, version, validate,
/// plan-publish, publish (dry-run), compose-pr-body, and completions.
#[test]
fn subprocess_lifecycle_exercises_every_main_dispatch_arm() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let root_str = root.to_string_lossy().to_string();

    let run = |args: &[&str]| -> std::process::Output {
        Command::new(bin())
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn callisto {args:?}: {e}"))
    };

    let init = run(&["--cwd", &root_str, "--format", "json", "init", "--yes"]);
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let status = run(&["--cwd", &root_str, "--format", "json", "status"]);
    assert!(
        status.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );

    let add = run(&[
        "--cwd",
        &root_str,
        "--format",
        "json",
        "add",
        "--package",
        "core-crate:patch",
        "--summary",
        "Fix a bug",
    ]);
    assert!(
        add.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    let validate = run(&["--cwd", &root_str, "--format", "json", "validate"]);
    assert!(
        validate.status.success(),
        "validate failed: {}",
        String::from_utf8_lossy(&validate.stderr)
    );

    let plan_publish = run(&["--cwd", &root_str, "--format", "json", "plan-publish"]);
    assert!(
        plan_publish.status.success(),
        "plan-publish failed: {}",
        String::from_utf8_lossy(&plan_publish.stderr)
    );

    // `--dry-run` so publish never attempts a real network call.
    let publish = run(&["--cwd", &root_str, "--format", "json", "--dry-run", "publish"]);
    assert!(
        publish.status.success(),
        "publish --dry-run failed: {}",
        String::from_utf8_lossy(&publish.stderr)
    );

    let compose = run(&["--cwd", &root_str, "--format", "json", "compose-pr-body"]);
    assert!(
        compose.status.success(),
        "compose-pr-body failed: {}",
        String::from_utf8_lossy(&compose.stderr)
    );

    let version = run(&["--cwd", &root_str, "--format", "json", "--dry-run", "version"]);
    assert!(
        version.status.success(),
        "version --dry-run failed: {}",
        String::from_utf8_lossy(&version.stderr)
    );

    let completions = run(&["completions", "bash"]);
    assert!(
        completions.status.success(),
        "completions failed: {}",
        String::from_utf8_lossy(&completions.stderr)
    );
    assert!(!completions.stdout.is_empty(), "completions must print a script");
}

/// `callisto tag --plan -` must read the publish plan from stdin.
#[test]
fn tag_plan_dash_reads_from_stdin() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let root_str = root.to_string_lossy().to_string();

    let init = Command::new(bin())
        .args(["--cwd", &root_str, "--format", "json", "init", "--yes"])
        .output()
        .unwrap();
    assert!(init.status.success());

    let plan_json = serde_json::json!({
        "schemaVersion": 1,
        "rustCrates": [],
        "npmPlatformPackages": [],
        "npmMainPackages": [],
        "releases": []
    })
    .to_string();

    let mut child = Command::new(bin())
        .args([
            "--cwd",
            &root_str,
            "--format",
            "json",
            "--dry-run",
            "tag",
            "--plan",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    child.stdin.take().unwrap().write_all(plan_json.as_bytes()).unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "tag --plan - failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `callisto compose-pr-body --existing-body -` must read the existing PR
/// body from stdin, stripping a leading BOM if present.
#[test]
fn compose_pr_body_existing_body_dash_reads_from_stdin() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let root_str = root.to_string_lossy().to_string();

    let init = Command::new(bin())
        .args(["--cwd", &root_str, "--format", "json", "init", "--yes"])
        .output()
        .unwrap();
    assert!(init.status.success());

    let add = Command::new(bin())
        .args([
            "--cwd",
            &root_str,
            "--format",
            "json",
            "add",
            "--package",
            "core-crate:patch",
            "--summary",
            "Fix a bug",
        ])
        .output()
        .unwrap();
    assert!(add.status.success());

    let mut child = Command::new(bin())
        .args([
            "--cwd",
            &root_str,
            "--format",
            "json",
            "compose-pr-body",
            "--existing-body",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // `render_pr_body_from_plan` preserves only the text preceding a prior
    // "## Release Preview" heading (custom notes a maintainer added above
    // Callisto's own generated section) -- it does not preserve arbitrary
    // existing body content. The leading BOM must be stripped before that
    // split happens, so it must not survive into the composed body either.
    child
        .stdin
        .take()
        .unwrap()
        .write_all("\u{FEFF}Custom maintainer note.\n\n## Release Preview\n\nStale content.\n".as_bytes())
        .unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "compose-pr-body --existing-body - failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let body = json["body"].as_str().expect("body must be a string");
    assert!(
        body.contains("Custom maintainer note."),
        "composed body must preserve the prefix above '## Release Preview' from the stdin-supplied existing body, got:\n{body}"
    );
    assert!(
        !body.contains('\u{FEFF}'),
        "leading BOM must be stripped before composing, got:\n{body}"
    );
    assert!(
        !body.contains("Stale content."),
        "content at or after '## Release Preview' in the existing body must not survive, got:\n{body}"
    );
}
