//! End-to-end coverage for the severity-validation message fix: before it,
//! `callisto add --package name:severity` claimed the allowed values were
//! "patch, minor, or major", omitting `none` even though `Severity` has
//! always accepted it. The unit tests next to the fix (`add.rs`) call
//! `parse_severity` directly; these prove the corrected wording -- and the
//! previously-undocumented `none` acceptance -- reach the real CLI's actual
//! stderr/stdout, exactly what an operator sees when they mistype a
//! severity.
//!
//! The lock-contention and identity-resolution fixes in the same PR are not
//! covered here: both are already exercised against a *real* OS-level
//! primitive (a real `fs2` file lock; a real malformed-manifest parse) in
//! their unit tests, and neither has a simple, direct CLI trigger --
//! `IdentityResolver::resolve`'s error paths are graph-internal, and
//! reproducing real lock contention at the CLI level requires a second
//! concurrent process. Noting the gap explicitly rather than skipping it in
//! silence, per this repo's sibling-gap discipline.

use std::path::Path;
use std::process::Command;

fn callisto_bin() -> &'static str {
    env!("CARGO_BIN_EXE_callisto")
}

fn setup_minimal_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    for args in [
        vec!["init", "-q", "-b", "main"],
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

    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/demo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("crates/demo/src")).unwrap();
    std::fs::write(
        root.join("crates/demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.join("crates/demo/src/lib.rs"), "pub fn hello() {}\n").unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    assert!(Command::new("git")
        .args(["add", "-A"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "Initial commit"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());

    dir
}

fn run_add(root: &Path, package_spec: &str) -> std::process::Output {
    Command::new(callisto_bin())
        .arg("--cwd")
        .arg(root)
        .args(["add", "--package", package_spec, "--summary", "test"])
        .output()
        .expect("failed to spawn callisto binary")
}

/// Error case: an invalid severity must be rejected, and the corrected
/// message must list every value `Severity` actually accepts -- including
/// `none`, which the pre-fix message omitted.
#[test]
fn add_rejects_invalid_severity_with_the_full_accepted_value_list() {
    let dir = setup_minimal_workspace();
    let output = run_add(dir.path(), "demo:bogus");

    assert!(!output.status.success(), "an invalid severity must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Must be none, patch, minor, or major"),
        "stderr must list all four accepted severities, including `none`: {stderr}"
    );
}

/// Happy/corner-case path: `none` has always been a valid severity value at
/// the parser level; this proves it still round-trips end to end through
/// the real CLI (not just through the previously-inaccurate error message).
#[test]
fn add_accepts_none_severity() {
    let dir = setup_minimal_workspace();
    let output = run_add(dir.path(), "demo:none");

    assert!(
        output.status.success(),
        "`none` must be accepted as a valid severity: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Added changeset"),
        "expected a changeset to be written: {stdout}"
    );
}

/// The three ordinary severities must keep working unchanged (regression
/// guard around the corrected message/parse path).
#[test]
fn add_accepts_patch_minor_major() {
    let dir = setup_minimal_workspace();
    for severity in ["patch", "minor", "major"] {
        let output = run_add(dir.path(), &format!("demo:{severity}"));
        assert!(
            output.status.success(),
            "`{severity}` must still be accepted: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
