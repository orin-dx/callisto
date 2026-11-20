#![cfg(unix)]

//! A whole prerelease cycle through the release pull request: each `release-pr decide` opens a PR only when the
//! cycle has something new, and each merged version commit verifies as release authority.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;

use release_harness::*;

/// The action `release-pr decide` takes, with the managed release PR open or not.
fn decide(root: &Path, release_pr_open: bool) -> String {
    let base = git(root, &["rev-parse", "HEAD"]);
    let open = if release_pr_open {
        format!(
            r#"[{{"number":7,"headRepository":"example/core-crate","headBranch":"callisto/version-packages","headCommit":"{}"}}]"#,
            "b".repeat(40)
        )
    } else {
        "[]".to_string()
    };
    let snapshot = format!(
        r#"{{"schemaVersion":2,"repository":"example/core-crate","baseBranch":"main","baseCommit":"{base}","openPullRequests":{open}}}"#
    );
    let out = callisto(
        root,
        &[
            "release-pr",
            "decide",
            "--snapshot",
            &snapshot,
            "--repository",
            "example/core-crate",
            "--base-branch",
            "main",
            "--release-branch",
            "callisto/version-packages",
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    report["action"]["kind"].as_str().unwrap().to_string()
}

/// Opens, merges, verifies and tags one release PR, as the release workflow's execute job would; returns the version
/// it released.
fn release(root: &Path) -> String {
    assert_eq!(decide(root, false), "create");
    assert_eq!(
        decide(root, true),
        "update",
        "a new change refreshes the open release PR"
    );
    let version = callisto(
        root,
        &["version", "--no-refresh-lockfiles", "--emit-decision", DECISION_PATH],
    );
    assert!(version.status.success(), "{}", String::from_utf8_lossy(&version.stderr));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "chore(release): version packages"]);
    let commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "-q", "--detach", &commit]);
    let external = tempfile::tempdir().unwrap();
    let plan = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &commit,
            "--decision",
            DECISION_PATH,
            "--out",
            external.path().join("intent.json").to_str().unwrap(),
        ],
    );
    assert!(plan.status.success(), "{}", String::from_utf8_lossy(&plan.stderr));
    git(root, &["checkout", "-q", "main"]);
    git(root, &["merge", "-q", "--ff-only", &commit]);
    let decision: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(DECISION_PATH)).unwrap()).unwrap();
    let version = decision["entries"][0]["targetVersion"].as_str().unwrap().to_string();
    git(
        root,
        &["tag", "-m", "release", &format!("core-crate@{version}"), &commit],
    );
    version
}

fn commit_change(root: &Path, message: &str) {
    fs::write(root.join("src/lib.rs"), format!("// {message}\npub fn core() {{}}\n")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

fn add(root: &Path, bump: &str, summary: &str) {
    let out = callisto(
        root,
        &[
            "add",
            "--package",
            &format!("cargo/core-crate:{bump}"),
            "--summary",
            summary,
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", summary]);
}

fn enter_v2(root: &Path) {
    let enter = callisto(root, &["pre", "enter", "beta"]);
    assert!(enter.status.success(), "{}", String::from_utf8_lossy(&enter.stderr));
}

fn enter_v3(root: &Path) {
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/pre.json"),
        "{\"mode\":\"pre\",\"tag\":\"beta\"}\n",
    )
    .unwrap();
}

/// Changesets v2 pre state: `pre enter` records the cycle's initial versions and applied changesets in `pre.json`.
#[test]
fn a_changesets_v2_prerelease_cycle_runs_end_to_end_through_release_pull_requests() {
    prerelease_cycle(Trigger::Changeset, enter_v2);
}

/// Changesets v3 pre state: `pre.json` holds only `mode` and `tag`, and applied notes move to `.changeset/pre/`.
#[test]
fn a_changesets_v3_prerelease_cycle_runs_end_to_end_through_release_pull_requests() {
    prerelease_cycle(Trigger::Changeset, enter_v3);
}

/// A package released from conventional commits runs the same cycle with no changesets at all.
#[test]
fn a_commit_inferred_prerelease_cycle_runs_end_to_end_through_release_pull_requests() {
    prerelease_cycle(Trigger::Commits, enter_v2);
    prerelease_cycle(Trigger::Commits, enter_v3);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Changeset,
    Commits,
}

fn prerelease_cycle(trigger: Trigger, enter: fn(&Path)) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/core-crate.git"],
    );
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn core() {}\n").unwrap();
    let release_trigger = match trigger {
        Trigger::Changeset => "",
        Trigger::Commits => "release-trigger = \"auto\"\n",
    };
    fs::write(
        root.join("callisto.toml"),
        format!("[[package]]\nmatch = \"cargo/core-crate\"\npublish-to = [\"crates-io\"]\n{release_trigger}"),
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "chore: initial"]);
    git(root, &["tag", "-m", "release", "core-crate@1.0.0"]);

    enter(root);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "chore: enter beta"]);
    assert_eq!(
        decide(root, false),
        "noop",
        "entering a cycle releases nothing on its own"
    );

    let change = |bump: &str, message: &str| match trigger {
        Trigger::Changeset => add(root, bump, message),
        Trigger::Commits => commit_change(root, message),
    };
    change("minor", "feat: add a feature");
    assert_eq!(release(root), "1.1.0-beta.0");

    change("patch", "fix: repair the feature");
    assert_eq!(release(root), "1.1.0-beta.1");

    assert_eq!(
        decide(root, false),
        "noop",
        "a cycle with nothing new opens no pull request"
    );

    let exit = callisto(root, &["pre", "exit"]);
    assert!(exit.status.success(), "{}", String::from_utf8_lossy(&exit.stderr));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "chore: exit beta"]);
    assert_eq!(release(root), "1.1.0");
    assert!(!root.join(".changeset/pre.json").exists());

    assert_eq!(decide(root, false), "noop");
}
