//! A failing command's JSON error carries the same `command` and `dryRun` as its success report,
//! and stderr is that JSON and nothing else.
#[path = "common/release_harness.rs"]
mod release_harness;

use release_harness::*;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn envelope_in(dir: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let out = callisto(dir, args);
    assert_eq!(out.status.code(), Some(1), "args {args:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    serde_json::from_str(&stderr)
        .unwrap_or_else(|error| panic!("stderr of {args:?} is not only JSON ({error}): {stderr}"))
}

fn error_envelope(args: &[&str]) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    callisto_fixtures::git::init_repo(dir.path());
    envelope_in(dir.path(), args)
}

fn assert_envelope(args: &[&str], command: &str) {
    let envelope = error_envelope(args);
    assert_eq!(envelope["command"], command, "{args:?}: {envelope}");
    assert_eq!(envelope["dryRun"], false, "{args:?}: {envelope}");
    assert!(envelope["error"]["code"].is_string(), "{args:?}: {envelope}");
}

#[test]
fn every_failing_release_pr_subcommand_reports_its_own_name() {
    assert_envelope(
        &["release-pr", "verify", "--decision", "{}", "--snapshot", "{}"],
        "release-pr verify",
    );
    assert_envelope(
        &["release-pr", "commit-plan", "--base-commit", SHA, "--message", "m"],
        "release-pr commit-plan",
    );
    assert_envelope(
        &[
            "release-pr",
            "decide",
            "--snapshot",
            "{}",
            "--repository",
            "o/r",
            "--base-branch",
            "main",
            "--release-branch",
            "callisto/version-packages",
        ],
        "release-pr decide",
    );
}

#[test]
fn every_failing_release_subcommand_reports_its_own_name() {
    assert_envelope(
        &["release", "plan", "--package", "cargo/a", "--out", "intent.json"],
        "release plan",
    );
    assert_envelope(
        &[
            "release",
            "execute",
            "--intent",
            "missing.json",
            "--receipt",
            "receipt.json",
            "--orchestration-revision",
            SHA,
        ],
        "release execute",
    );
    assert_envelope(
        &[
            "release",
            "artifact-manifest",
            "--intent",
            "missing.json",
            "--artifact-dir",
            "dist",
            "--out",
            "manifest.json",
        ],
        "release artifact-manifest",
    );
}

#[test]
fn a_failing_dry_run_reports_dry_run_true() {
    let envelope = error_envelope(&["--dry-run", "version"]);
    assert_eq!(envelope["command"], "version", "{envelope}");
    assert_eq!(envelope["dryRun"], true, "{envelope}");
}

// Outside a repository the tool's own "not a git repository" message must not be echoed ahead of the JSON.
#[test]
fn outside_a_repository_stderr_is_still_only_the_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let envelope = envelope_in(dir.path(), &["status"]);
    assert_eq!(envelope["command"], "status", "{envelope}");
}
