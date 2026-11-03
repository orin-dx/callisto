//! REL-CMD-12 and REL-PR-07: a failing command's JSON error carries the same `command` and `dryRun` as its success report.
#[path = "common/release_harness.rs"]
mod release_harness;

use release_harness::*;

fn error_envelope(args: &[&str]) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    callisto_fixtures::git::init_repo(dir.path());
    let out = callisto(dir.path(), args);
    assert_eq!(out.status.code(), Some(1), "args {args:?}");
    // A failing git call can echo its own message before the envelope; the envelope is what starts at the first `{`.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let start = stderr
        .find('{')
        .unwrap_or_else(|| panic!("no JSON in stderr of {args:?}: {stderr}"));
    serde_json::from_str(&stderr[start..])
        .unwrap_or_else(|error| panic!("stderr of {args:?} is not JSON ({error}): {stderr}"))
}

#[test]
fn a_failing_release_pr_subcommand_reports_its_name_and_dry_run() {
    let envelope = error_envelope(&["release-pr", "verify", "--decision", "{}", "--snapshot", "{}"]);
    assert_eq!(envelope["command"], "release-pr verify", "{envelope}");
    assert_eq!(envelope["dryRun"], false, "{envelope}");
    assert!(envelope["error"]["code"].is_string(), "{envelope}");

    let envelope = error_envelope(&[
        "release-pr",
        "commit-plan",
        "--base-commit",
        "0123456789abcdef0123456789abcdef01234567",
        "--message",
        "m",
    ]);
    assert_eq!(envelope["command"], "release-pr commit-plan", "{envelope}");
}

#[test]
fn a_failing_release_plan_reports_its_name() {
    let envelope = error_envelope(&["release", "plan", "--package", "cargo/a", "--out", "intent.json"]);
    assert_eq!(envelope["command"], "release plan", "{envelope}");
    assert_eq!(envelope["dryRun"], false, "{envelope}");
}

#[test]
fn a_failing_dry_run_reports_dry_run_true() {
    let envelope = error_envelope(&["--dry-run", "version"]);
    assert_eq!(envelope["command"], "version", "{envelope}");
    assert_eq!(envelope["dryRun"], true, "{envelope}");
}
